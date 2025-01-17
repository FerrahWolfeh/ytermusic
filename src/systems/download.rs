use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::Duration,
};

use flume::Sender;
use once_cell::sync::Lazy;
use rustube::{Error, Id};
use tokio::{task::JoinHandle, time::sleep};
use ytpapi::Video;

use crate::{consts::CACHE_DIR, SoundAction};

pub static IN_DOWNLOAD: Lazy<Mutex<Vec<ytpapi::Video>>> = Lazy::new(|| Mutex::new(Vec::new()));
static HANDLES: Lazy<Mutex<Vec<JoinHandle<()>>>> = Lazy::new(|| Mutex::new(Vec::new()));
pub static DOWNLOAD_MORE: AtomicBool = AtomicBool::new(true);
// TODO Maybe switch to a channel
static DOWNLOAD_QUEUE: Lazy<Mutex<VecDeque<ytpapi::Video>>> =
    Lazy::new(|| Mutex::new(VecDeque::new()));

fn take() -> Option<Video> {
    DOWNLOAD_QUEUE.lock().unwrap().pop_front()
}

pub fn clean(sender: Arc<Sender<SoundAction>>) {
    DOWNLOAD_QUEUE.lock().unwrap().clear();
    {
        let mut handle = HANDLES.lock().unwrap();
        for i in handle.iter() {
            i.abort()
        }
        handle.clear();
    }
    IN_DOWNLOAD.lock().unwrap().clear();
    DOWNLOAD_MORE.store(true, std::sync::atomic::Ordering::SeqCst);
    downloader(sender);
}

pub fn add(video: Video, s: &Sender<SoundAction>) {
    let download_path_json = CACHE_DIR.join(&format!("downloads/{}.json", &video.video_id));
    if download_path_json.exists() {
        s.send(SoundAction::PlayVideo(video)).unwrap();
    } else {
        DOWNLOAD_QUEUE.lock().unwrap().push_back(video);
    }
}

async fn handle_download(id: &str) -> Result<PathBuf, Error> {
    rustube::Video::from_id(Id::from_str(id)?.into_owned())
        .await?
        .streams()
        .iter()
        .filter(|stream| {
            stream.mime == "audio/mp4"
                && stream.includes_audio_track
                && !stream.includes_video_track
        })
        .max_by_key(|stream| stream.bitrate)
        .ok_or(Error::NoStreams)?
        .download_to_dir(CACHE_DIR.join("downloads"))
        .await
}

const DOWNLOADER_COUNT: usize = 4;

pub fn start_task(s: Arc<Sender<SoundAction>>) {
    HANDLES.lock().unwrap().push(tokio::task::spawn(async move {
        let mut k = true;
        loop {
            if !k {
                sleep(Duration::from_millis(200)).await;
            } else {
                k = false;
            }
            if !DOWNLOAD_MORE.load(std::sync::atomic::Ordering::SeqCst) {
                continue;
            }
            if let Some(id) = take() {
                // TODO(#1): handle errors
                let download_path_mp4 = CACHE_DIR.join(&format!("downloads/{}.mp4", &id.video_id));
                let download_path_json =
                    CACHE_DIR.join(&format!("downloads/{}.json", &id.video_id));
                if download_path_json.exists() {
                    s.send(SoundAction::PlayVideo(id)).unwrap();
                    k = true;
                    continue;
                }
                if download_path_mp4.exists() {
                    std::fs::remove_file(&download_path_mp4).unwrap();
                }
                {
                    IN_DOWNLOAD.lock().unwrap().push(id.clone());
                }
                match handle_download(&id.video_id).await {
                    Ok(_) => {
                        std::fs::write(download_path_json, serde_json::to_string(&id).unwrap())
                            .unwrap();
                        crate::append(id.clone());
                        {
                            IN_DOWNLOAD
                                .lock()
                                .unwrap()
                                .retain(|x| x.video_id != id.video_id);
                        }
                        s.send(SoundAction::PlayVideo(id)).unwrap();
                        k = true;
                    }
                    Err(_) => {
                        if download_path_mp4.exists() {
                            std::fs::remove_file(download_path_mp4).unwrap();
                        }

                        {
                            IN_DOWNLOAD
                                .lock()
                                .unwrap()
                                .retain(|x| x.video_id != id.video_id);
                        }
                        // TODO(#1): handle errors
                    }
                }
            }
        }
    }));
}
pub fn start_task_unary(s: Arc<Sender<SoundAction>>, song: Video) {
    HANDLES.lock().unwrap().push(tokio::task::spawn(async move {
        let download_path_mp4 = CACHE_DIR.join(&format!("downloads/{}.mp4", &song.video_id));
        let download_path_json = CACHE_DIR.join(&format!("downloads/{}.json", &song.video_id));
        if download_path_json.exists() {
            s.send(SoundAction::PlayVideoUnary(song.clone())).unwrap();
            return;
        }
        if download_path_mp4.exists() {
            std::fs::remove_file(&download_path_mp4).unwrap();
        }
        {
            IN_DOWNLOAD.lock().unwrap().push(song.clone());
        }
        match handle_download(&song.video_id).await {
            Ok(_) => {
                std::fs::write(download_path_json, serde_json::to_string(&song).unwrap()).unwrap();
                crate::append(song.clone());
                {
                    IN_DOWNLOAD
                        .lock()
                        .unwrap()
                        .retain(|x| x.video_id != song.video_id);
                }
                s.send(SoundAction::PlayVideoUnary(song)).unwrap();
            }
            Err(_) => {
                if download_path_mp4.exists() {
                    std::fs::remove_file(download_path_mp4).unwrap();
                }

                {
                    IN_DOWNLOAD
                        .lock()
                        .unwrap()
                        .retain(|x| x.video_id != song.video_id);
                }
                // TODO(#1): handle errors
            }
        }
    }));
}

pub fn downloader(s: Arc<Sender<SoundAction>>) {
    for _ in 0..DOWNLOADER_COUNT {
        start_task(s.clone());
    }
}
