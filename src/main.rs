use consts::CACHE_DIR;
use rustube::Error;
use term::{Manager, ManagerMessage, Screens};

use std::collections::HashSet;
use std::{path::PathBuf, str::FromStr, sync::Arc};
use systems::download::downloader;
use systems::player::player_system;

use ytpapi::{Video, YTApi};

use crate::consts::HEADER_TUTORIAL;
use crate::systems::logger::log_;

mod consts;
mod database;
mod errors;
mod systems;
mod term;

pub use database::*;

use mimalloc::MiMalloc;

// Changes the allocator to improve performance especially on Windows
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/**
 * Actions that can be sent to the player from other services
 */
#[derive(Debug, Clone)]
pub enum SoundAction {
    Cleanup,
    PlayPause,
    ForcePause,
    ForcePlay,
    RestartPlayer,
    Plus,
    Minus,
    Previous(usize),
    Forward,
    Backward,
    Next(usize),
    PlayVideo(Video),
    PlayVideoUnary(Video),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    std::fs::write("log.txt", "# YTerMusic log file\n\n").unwrap();
    std::fs::create_dir_all(CACHE_DIR.join("downloads")).unwrap();
    if !PathBuf::from_str("headers.txt").unwrap().exists() {
        println!("The `headers.txt` file is not present in the root directory.");
        println!("{}", HEADER_TUTORIAL);
        return Ok(());
    }
    if !std::fs::read_to_string("headers.txt")
        .unwrap()
        .contains("Cookie: ")
    {
        println!("The `headers.txt` file is not configured correctly.");
        println!("{}", HEADER_TUTORIAL);
        return Ok(());
    }

    log_("Starting YTerMusic");

    // Spawn the clean task
    let (updater_s, updater_r) = flume::unbounded::<ManagerMessage>();
    std::thread::spawn(move || {
        log_("Cleaning service on");
        clean();
    });
    let updater_s = Arc::new(updater_s);
    // Spawn the player task
    let (sa, player) = player_system(updater_s.clone());
    // Spawn the downloader task
    downloader(sa.clone());
    {
        let updater_s = updater_s.clone();
        // Spawn playlist updater task
        tokio::task::spawn(async move {
            log_("Last playlist task on");
            let playlist = std::fs::read_to_string(CACHE_DIR.join("last-playlist.json")).ok()?;
            let mut playlist: (String, Vec<Video>) = serde_json::from_str(&playlist).ok()?;
            if !playlist.0.starts_with("Last playlist: ") {
                playlist.0 = format!("Last playlist: {}", playlist.0);
            }
            updater_s
                .send(ManagerMessage::AddElementToChooser(playlist).pass_to(Screens::Playlist))
                .unwrap();
            Some(())
        });
    }
    {
        let updater_s = updater_s.clone();
        // Spawn the API task
        tokio::task::spawn(async move {
            log_("API task on");
            match YTApi::from_header_file(PathBuf::from_str("headers.txt").unwrap().as_path()).await
            {
                Ok(api) => {
                    let api = Arc::new(api);
                    for playlist in api.playlists() {
                        let updater_s = updater_s.clone();
                        let playlist = playlist.clone();
                        let api = api.clone();
                        tokio::task::spawn(async move {
                            match api.browse_playlist(&playlist.browse_id).await {
                                Ok(videos) => {
                                    updater_s
                                        .send(
                                            ManagerMessage::AddElementToChooser((
                                                format!(
                                                    "{} ({})",
                                                    playlist.name, playlist.subtitle
                                                ),
                                                videos,
                                            ))
                                            .pass_to(Screens::Playlist),
                                        )
                                        .unwrap();
                                }
                                Err(e) => {
                                    log_(format!("{:?}", e));
                                }
                            }
                        });
                    }
                }
                Err(e) => {
                    log_(format!("{:?}", e));
                }
            }
        });
    }
    {
        let updater_s = updater_s.clone();
        // Spawn the database getter task
        tokio::task::spawn(async move {
            log_("Database getter task on");
            if let Some(e) = read() {
                *DATABASE.write().unwrap() = e.clone();

                updater_s
                    .send(
                        ManagerMessage::AddElementToChooser(("Local musics".to_owned(), e))
                            .pass_to(Screens::Playlist),
                    )
                    .unwrap();
            } else {
                let mut videos = HashSet::new();
                for files in std::fs::read_dir(CACHE_DIR.join("downloads")).unwrap() {
                    let path = files.unwrap().path();
                    if path.as_os_str().to_string_lossy().ends_with(".json") {
                        let video =
                            serde_json::from_str(std::fs::read_to_string(path).unwrap().as_str())
                                .unwrap();
                        videos.insert(video);
                    }
                }

                let k = videos.into_iter().collect::<Vec<_>>();

                *DATABASE.write().unwrap() = k.clone();

                updater_s
                    .send(
                        ManagerMessage::AddElementToChooser(("Local musics".to_owned(), k))
                            .pass_to(Screens::Playlist),
                    )
                    .unwrap();
                write();
            }
        });
    }

    log_("Running the manager");
    let mut manager = Manager::new(sa, player).await;
    manager.run(&updater_r).unwrap();
    Ok(())
}

/**
 * This function is called on start to clean the database and the files that are incompletly downloaded due to a crash.
 */
fn clean() {
    for i in std::fs::read_dir(CACHE_DIR.join("downloads")).unwrap() {
        let path = i.unwrap().path();
        if path.ends_with(".mp4") {
            let mut path1 = path.clone();
            path1.set_extension("json");
            if !path1.exists() {
                std::fs::remove_file(&path).unwrap();
            }
        }
    }
}
