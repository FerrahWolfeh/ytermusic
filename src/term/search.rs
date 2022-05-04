use std::{
    path::PathBuf,
    str::FromStr,
    sync::{Arc, RwLock},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
use flume::Sender;
use tokio::task::JoinHandle;
use tui::{
    layout::{Alignment, Rect},
    style::{Color, Style},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use urlencoding::encode;
use ytpapi::{Video, YTApi};

use crate::{
    systems::{download, logger::log},
    SoundAction,
};

use super::{
    rect_contains, relative_pos, split_y_start, EventResponse, ManagerMessage, Screen, Screens,
};

pub struct Search {
    pub text: String,
    pub selected: usize,
    pub items: Arc<RwLock<Vec<(String, Video)>>>,
    pub search_handle: Option<JoinHandle<()>>,
    pub api: Option<Arc<ytpapi::YTApi>>,
    pub action_sender: Arc<Sender<SoundAction>>,
}
impl Screen for Search {
    fn on_mouse_press(
        &mut self,
        mouse_event: crossterm::event::MouseEvent,
        frame_data: &Rect,
    ) -> EventResponse {
        if let MouseEventKind::Down(_) = mouse_event.kind {
            let splitted = split_y_start(*frame_data, 3);
            let x = mouse_event.column;
            let y = mouse_event.row;
            if rect_contains(&splitted[1], x, y, 1) {
                let (_, y) = relative_pos(&splitted[1], x, y, 1);
                let y = if self.selected == 0 {
                    y
                } else {
                    y + self.selected as u16 - 1
                };
                if self.items.read().unwrap().len() > y as usize {
                    self.selected = y as usize;
                    return self.on_key_press(
                        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                        frame_data,
                    );
                }
            }
        }
        EventResponse::None
    }

    fn on_key_press(&mut self, key: KeyEvent, _: &Rect) -> EventResponse {
        if KeyCode::Esc == key.code {
            return ManagerMessage::ChangeState(Screens::Playlist).event();
        }
        let textbefore = self.text.trim().to_owned();
        match key.code {
            KeyCode::Enter => {
                if let Some(a) = self.items.read().unwrap().get(self.selected).cloned() {
                    self.action_sender.send(SoundAction::Cleanup).unwrap();
                    download::clean(self.action_sender.clone());
                    download::add(a.1);
                    return ManagerMessage::ChangeState(Screens::MusicPlayer).event();
                }
            }
            KeyCode::Char('+') | KeyCode::Up => self.selected(self.selected as isize - 1),
            KeyCode::Char('-') | KeyCode::Down => self.selected(self.selected as isize + 1),
            KeyCode::Delete | KeyCode::Backspace => {
                self.text.pop();
            }
            KeyCode::Char(a) => {
                self.text.push(a);
            }
            _ => {}
        }
        if textbefore == self.text.trim() {
            return EventResponse::None;
        }

        if let Some(handle) = self.search_handle.take() {
            handle.abort();
        }

        if let Some(api) = self.api.clone() {
            let text = self.text.clone();
            let items = self.items.clone();
            self.selected = 0;
            self.search_handle = Some(tokio::task::spawn(async move {
                let mut item = Vec::new();
                // HANDLE ERRORS
                match api.search(&encode(&text).replace("%20", "+")).await {
                    Ok(e) => {
                        for video in e.into_iter() {
                            item.push((format!("{} | {}", video.author, video.title), video));
                        }
                    }
                    Err(e) => {
                        log(format!("{:?}", e));
                    }
                }
                items.write().unwrap().clear();
                items.write().unwrap().extend(item.into_iter());
            }));
        } else {
            self.set_elements(vec![]);
        }

        EventResponse::None
    }

    fn render(&mut self, frame: &mut Frame<tui::backend::CrosstermBackend<std::io::Stdout>>) {
        let splitted = split_y_start(frame.size(), 3);
        frame.render_widget(
            Paragraph::new(self.text.clone())
                .style(Style::default().fg(Color::LightCyan))
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .style(Style::default().fg(Color::White))
                        .title(" Search ")
                        .border_type(BorderType::Plain),
                ),
            splitted[0],
        );
        frame.render_stateful_widget(
            List::new(
                self.items
                    .read()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .skip(self.selected.saturating_sub(1))
                    .map(|(index, i)| {
                        ListItem::new(i.0.as_str()).style(
                            Style::default()
                                .fg(if index == self.selected {
                                    Color::Black
                                } else {
                                    Color::White
                                })
                                .bg(if index != self.selected {
                                    Color::Black
                                } else {
                                    Color::White
                                }),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Select the playlist to play "),
            ),
            splitted[1],
            &mut ListState::default(),
        );
    }

    fn handle_global_message(&mut self, _: super::ManagerMessage) -> EventResponse {
        EventResponse::None
    }

    fn close(&mut self, _: Screens) -> EventResponse {
        EventResponse::None
    }

    fn open(&mut self) -> EventResponse {
        EventResponse::None
    }
}
impl Search {
    pub async fn new(action_sender: Arc<Sender<SoundAction>>) -> Self {
        Self {
            text: String::new(),
            selected: 0,
            items: Arc::new(RwLock::new(Vec::new())),
            search_handle: None,
            api: YTApi::from_header_file(PathBuf::from_str("headers.txt").unwrap().as_path())
                .await
                .ok()
                .map(Arc::new),
            action_sender,
        }
    }
    fn selected(&mut self, selected: isize) {
        let k = self.items.read().unwrap().len();
        if selected < 0 {
            if k == 0 {
                self.selected = 0;
            } else {
                self.selected = k - 1;
            }
        } else if selected >= k as isize {
            self.selected = 0;
        } else {
            self.selected = selected as usize;
        }
    }
    fn set_elements(&mut self, element: Vec<(String, Video)>) {
        *self.items.write().unwrap() = element;
        self.selected = 0;
    }
}
