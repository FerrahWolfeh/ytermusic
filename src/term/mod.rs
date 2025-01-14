pub mod device_lost;
pub mod music_player;
pub mod playlist;
pub mod search;

use std::{
    io::{self, Stdout},
    sync::Arc,
    time::{Duration, Instant},
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyEvent, KeyModifiers, MouseEvent,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use flume::{Receiver, Sender};
use tui::{backend::CrosstermBackend, layout::Rect, Frame, Terminal};
use ytpapi::Video;

use crate::{systems::player::PlayerState, SoundAction};

use self::{device_lost::DeviceLost, playlist::Chooser, search::Search};

// A trait to handle the different screens
pub trait Screen {
    fn on_mouse_press(&mut self, mouse_event: MouseEvent, frame_data: &Rect) -> EventResponse;
    fn on_key_press(&mut self, mouse_event: KeyEvent, frame_data: &Rect) -> EventResponse;
    fn render(&mut self, frame: &mut Frame<CrosstermBackend<Stdout>>);
    fn handle_global_message(&mut self, message: ManagerMessage) -> EventResponse;
    fn close(&mut self, new_screen: Screens) -> EventResponse;
    fn open(&mut self) -> EventResponse;
}

#[derive(Debug, Clone)]
pub enum EventResponse {
    Message(Vec<ManagerMessage>),
    None,
}

// A message that can be sent to the manager
#[derive(Debug, Clone)]
pub enum ManagerMessage {
    Error(String),
    PassTo(Screens, Box<ManagerMessage>),
    ChangeState(Screens),
    RestartPlayer,
    Quit,
    AddElementToChooser((String, Vec<Video>)),
}

impl ManagerMessage {
    pub fn pass_to(self, screen: Screens) -> Self {
        Self::PassTo(screen, Box::new(self))
    }
    pub fn event(self) -> EventResponse {
        EventResponse::Message(vec![self])
    }
}

// The different screens
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Screens {
    MusicPlayer = 0x0,
    Playlist = 0x1,
    Search = 0x2,
    DeviceLost = 0x3,
}

// The screen manager that handles the different screens
pub struct Manager {
    music_player: PlayerState,
    chooser: Chooser,
    search: Search,
    device_lost: DeviceLost,
    current_screen: Screens,
}

impl Manager {
    pub async fn new(action_sender: Arc<Sender<SoundAction>>, music_player: PlayerState) -> Self {
        Self {
            music_player,
            chooser: Chooser {
                selected: 0,
                items: vec![],
                action_sender: action_sender.clone(),
            },
            search: Search::new(action_sender).await,
            current_screen: Screens::Playlist,
            device_lost: DeviceLost(Vec::new()),
        }
    }
    pub fn current_screen(&mut self) -> &mut dyn Screen {
        self.get_screen(self.current_screen)
    }
    pub fn get_screen(&mut self, screen: Screens) -> &mut dyn Screen {
        match screen {
            Screens::MusicPlayer => &mut self.music_player,
            Screens::Playlist => &mut self.chooser,
            Screens::Search => &mut self.search,
            Screens::DeviceLost => &mut self.device_lost,
        }
    }
    pub fn set_current_screen(&mut self, screen: Screens) {
        self.current_screen = screen;
        let k = self.current_screen().open();
        self.handle_event(k);
    }
    pub fn handle_event(&mut self, event: EventResponse) -> bool {
        match event {
            EventResponse::Message(messages) => {
                for message in messages {
                    if self.handle_manager_message(message) {
                        return true;
                    }
                }
            }
            EventResponse::None => {}
        }
        false
    }
    pub fn handle_manager_message(&mut self, e: ManagerMessage) -> bool {
        match e {
            ManagerMessage::PassTo(e, a) => {
                let rs = self.get_screen(e).handle_global_message(*a);
                self.handle_event(rs);
            }
            ManagerMessage::Quit => {
                let c = self.current_screen;
                self.current_screen().close(c);
                return true;
            }
            ManagerMessage::ChangeState(e) => {
                self.current_screen().close(e);
                self.set_current_screen(e);
            }
            e => {
                return self.handle_manager_message(ManagerMessage::PassTo(
                    Screens::DeviceLost,
                    Box::new(ManagerMessage::Error(format!(
                        "Invalid manager message (Forward the message to a screen maybe):\n{:?}",
                        e
                    ))),
                ));
            }
        }
        false
    }
    /**
     * The main loop of the manager
     */
    pub fn run(&mut self, updater: &Receiver<ManagerMessage>) -> Result<(), io::Error> {
        // setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // create app and run it
        let tick_rate = Duration::from_millis(250);

        let mut last_tick = Instant::now();
        'a: loop {
            while let Ok(e) = updater.try_recv() {
                if self.handle_manager_message(e) {
                    break 'a;
                }
            }
            let rectsize = terminal.size()?;
            terminal.draw(|f| {
                self.current_screen().render(f);
            })?;

            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));
            if crossterm::event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) => {
                        if (key.code == event::KeyCode::Char('c')
                            || key.code == event::KeyCode::Char('d'))
                            && key.modifiers == KeyModifiers::CONTROL
                        {
                            break;
                        }
                        let k = self.current_screen().on_key_press(key, &rectsize);
                        if self.handle_event(k) {
                            break;
                        }
                    }
                    Event::Mouse(mouse) => {
                        let k = self.current_screen().on_mouse_press(mouse, &rectsize);
                        if self.handle_event(k) {
                            break;
                        }
                    }
                    _ => (),
                }
            }
            if last_tick.elapsed() >= tick_rate {
                last_tick = Instant::now();
            }
        }

        // restore terminal
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        Ok(())
    }
}

// UTILS SECTION TO SPLIT THE TERMINAL INTO DIFFERENT PARTS

pub fn split_y_start(f: Rect, start_size: u16) -> [Rect; 2] {
    let mut rectlistvol = f;
    rectlistvol.height = start_size;
    let mut rectprogress = f;
    rectprogress.y += start_size;
    rectprogress.height -= start_size;
    [rectlistvol, rectprogress]
}
pub fn split_y(f: Rect, end_size: u16) -> [Rect; 2] {
    let mut rectlistvol = f;
    rectlistvol.height -= end_size;
    let mut rectprogress = f;
    rectprogress.y += rectprogress.height - end_size;
    rectprogress.height = end_size;
    [rectlistvol, rectprogress]
}
pub fn split_x(f: Rect, end_size: u16) -> [Rect; 2] {
    let mut rectlistvol = f;
    rectlistvol.width -= end_size;
    let mut rectprogress = f;
    rectprogress.x += rectprogress.width - end_size;
    rectprogress.width = end_size;
    [rectlistvol, rectprogress]
}

pub fn rect_contains(rect: &Rect, x: u16, y: u16, margin: u16) -> bool {
    rect.x + margin <= x
        && x <= rect.x + rect.width - margin
        && rect.y + margin <= y
        && y <= rect.y + rect.height - margin
}

pub fn relative_pos(rect: &Rect, x: u16, y: u16, margin: u16) -> (u16, u16) {
    (x - rect.x - margin, y - rect.y - margin)
}
