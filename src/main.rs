use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Bar, BarChart, BarGroup, Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use std::{
    io,
    process::{Child, Command},
    time::{Duration, Instant},
};

mod api;

const RADIO_BROWSER_BASE: &str = "https://de1.api.radio-browser.info";

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Station {
    pub name: String,
    #[serde(rename = "url_resolved")]
    pub url: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub bitrate: u32,
}

#[derive(Debug)]
struct Playing {
    station: Station,
    child: Child,
    volume: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Discover,
    Search,
}

struct App {
    stations: Vec<Station>,
    list_state: ListState,
    search_query: String,
    current_tab: Tab,
    playing: Option<Playing>,
    viz_phase: f64,
    viz_bars: Vec<u8>,
    status: String,
    discover_tags: Vec<&'static str>,
    current_tag_filter: Option<usize>,
    favorites: Vec<Station>,
}

impl App {
    fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Self {
            stations: vec![],
            list_state,
            search_query: String::new(),
            current_tab: Tab::Discover,
            playing: None,
            viz_phase: 0.0,
            viz_bars: vec![5; 24],
            status: "Loading stations...".to_string(),
            discover_tags: vec![
                "lofi",
                "jazz",
                "classical",
                "electronic",
                "ambient",
                "rock",
                "pop",
                "hiphop",
                "news",
                "talk",
                "soul",
                "funk",
                "metal",
                "blues",
                "country",
                "reggae",
            ],
            current_tag_filter: None,
            favorites: vec![],
        }
    }

    fn selected_station(&self) -> Option<Station> {
        let filtered = self.filtered_stations();
        self.list_state
            .selected()
            .and_then(|i| filtered.get(i).cloned())
    }

    fn filtered_stations(&self) -> Vec<Station> {
        let q = self.search_query.to_lowercase();
        let tag = self
            .current_tag_filter
            .and_then(|i| self.discover_tags.get(i));

        self.stations
            .iter()
            .filter(|s| {
                let matches_search = q.is_empty()
                    || s.name.to_lowercase().contains(&q)
                    || s.tags.to_lowercase().contains(&q)
                    || s.country.to_lowercase().contains(&q);

                let matches_tag = tag
                    .map(|t| {
                        s.tags.to_lowercase().contains(*t) || s.name.to_lowercase().contains(*t)
                    })
                    .unwrap_or(true);

                matches_search && matches_tag
            })
            .cloned()
            .collect()
    }

    fn next(&mut self) {
        let len = self.filtered_stations().len();
        if len == 0 {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => (i + 1) % len,
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn previous(&mut self) {
        let len = self.filtered_stations().len();
        if len == 0 {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    len - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn play_selected(&mut self) {
        if let Some(station) = self.selected_station() {
            self.stop_playing();

            let volume = 70;
            match spawn_player(&station.url, volume) {
                Ok(child) => {
                    self.playing = Some(Playing {
                        station: station.clone(),
                        child,
                        volume,
                    });
                    self.status = format!("▶ Playing: {}", station.name);
                    // kick viz
                    self.viz_phase = 0.0;
                }
                Err(e) => {
                    self.status = format!("Failed to play: {}", e);
                }
            }
        }
    }

    fn stop_playing(&mut self) {
        if let Some(mut p) = self.playing.take() {
            let _ = p.child.kill();
            let _ = p.child.wait();
            self.status = format!("⏹ Stopped: {}", p.station.name);
        }
    }

    fn adjust_volume(&mut self, delta: i32) {
        if let Some(p) = &mut self.playing {
            let new_vol = (p.volume as i32 + delta).clamp(0, 100) as u8;
            if new_vol != p.volume {
                p.volume = new_vol;
                // restart player with new volume (simple approach)
                let url = p.station.url.clone();
                let _ = p.child.kill();
                let _ = p.child.wait();
                match spawn_player(&url, new_vol) {
                    Ok(new_child) => {
                        p.child = new_child;
                        self.status = format!("▶ {} (vol {})", p.station.name, new_vol);
                    }
                    Err(e) => {
                        self.status = format!("Volume change failed: {}", e);
                    }
                }
            }
        }
    }

    fn toggle_tab(&mut self) {
        self.current_tab = match self.current_tab {
            Tab::Discover => Tab::Search,
            Tab::Search => Tab::Discover,
        };
        // reset filter when going to search tab
        if self.current_tab == Tab::Search {
            self.current_tag_filter = None;
        }
        self.refresh_filter();
    }

    fn cycle_tag_filter(&mut self) {
        if self.current_tab != Tab::Discover {
            return;
        }
        let next = match self.current_tag_filter {
            None => Some(0),
            Some(i) => {
                if i + 1 >= self.discover_tags.len() {
                    None
                } else {
                    Some(i + 1)
                }
            }
        };
        self.current_tag_filter = next;
        self.refresh_filter();
    }

    fn refresh_filter(&mut self) {
        // keep selection in bounds
        let len = self.filtered_stations().len();
        if len == 0 {
            self.list_state.select(None);
        } else if let Some(sel) = self.list_state.selected() {
            if sel >= len {
                self.list_state.select(Some(len - 1));
            }
        } else {
            self.list_state.select(Some(0));
        }
    }

    fn update_viz(&mut self, playing: bool) {
        self.viz_phase += 0.18;

        for (i, bar) in self.viz_bars.iter_mut().enumerate() {
            let wave = (self.viz_phase + i as f64 * 0.6).sin() * 0.5 + 0.5;
            let energy = if playing { 0.75 } else { 0.15 };
            let noise = ((i as f64 * 1.3 + self.viz_phase * 1.2).sin() * 0.5 + 0.5) * 0.3;
            let val = (wave * energy + noise) * 18.0 + 3.0;
            *bar = val.clamp(1.0, 22.0) as u8;
        }
    }
}

fn spawn_player(url: &str, volume: u8) -> Result<Child> {
    // Prefer ffplay (already present), fall back hints for mpv
    let mut cmd = Command::new("ffplay");
    cmd.args([
        "-nodisp",
        "-autoexit",
        "-volume",
        &volume.to_string(),
        "-loglevel",
        "quiet",
        url,
    ]);

    match cmd.spawn() {
        Ok(child) => Ok(child),
        Err(_) => {
            // Try mpv as nicer alternative
            let mut mpv = Command::new("mpv");
            mpv.args([
                "--no-video",
                "--really-quiet",
                &format!("--volume={}", volume),
                url,
            ]);
            mpv.spawn().map_err(|e| {
                anyhow::anyhow!(
                    "Could not launch ffplay or mpv. Install one:\n  brew install ffmpeg   (for ffplay)\n  brew install mpv"
                )
                .context(e)
            })
        }
    }
}

fn main() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    // Initial data load (blocking is fine for startup)
    match api::fetch_top_stations(80) {
        Ok(sts) => {
            app.stations = sts;
            app.status =
                "Loaded stations from Radio Browser. Press Enter to play, / to filter.".into();
        }
        Err(e) => {
            app.status = format!("Failed to load stations: {}. Using demo list.", e);
            app.stations = api::demo_stations();
        }
    }

    // Ctrl+C handler to clean up player
    {
        let _ = ctrlc::set_handler(|| {
            // Best effort cleanup on abrupt exit
            std::process::exit(0);
        });
    };

    let res = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Make sure player is dead
    if let Some(mut p) = app.playing.take() {
        let _ = p.child.kill();
    }

    res
}

fn run_app<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let tick_rate = Duration::from_millis(80);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| ui(f, app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => {
                            return Ok(());
                        }
                        KeyCode::Esc => {
                            if !app.search_query.is_empty() {
                                app.search_query.clear();
                                app.refresh_filter();
                            } else {
                                return Ok(());
                            }
                        }
                        KeyCode::Char('c')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            return Ok(());
                        }
                        KeyCode::Down | KeyCode::Char('j') => app.next(),
                        KeyCode::Up | KeyCode::Char('k') => app.previous(),
                        KeyCode::Enter => app.play_selected(),
                        KeyCode::Char('s') | KeyCode::Char(' ') => app.stop_playing(),
                        KeyCode::Char('+') | KeyCode::Char('=') => app.adjust_volume(10),
                        KeyCode::Char('-') => app.adjust_volume(-10),
                        KeyCode::Char('/') => {
                            if app.search_query.is_empty() {
                                app.current_tab = Tab::Search;
                            } else {
                                app.search_query.clear();
                            }
                            app.refresh_filter();
                        }
                        KeyCode::Char('d') => {
                            app.cycle_tag_filter();
                        }
                        KeyCode::Char('f') => {
                            if let Some(st) = app.selected_station() {
                                if !app.favorites.iter().any(|s| s.url == st.url) {
                                    app.favorites.push(st.clone());
                                    app.status = format!("★ Favorited: {}", st.name);
                                }
                            }
                        }
                        KeyCode::Char('F') if !app.favorites.is_empty() => {
                            app.stations = app.favorites.clone();
                            app.current_tag_filter = None;
                            app.search_query.clear();
                            app.list_state.select(Some(0));
                            app.status = "Showing favorites (restart to reload all)".into();
                        }
                        KeyCode::Tab => {
                            app.toggle_tab();
                            app.search_query.clear();
                            app.refresh_filter();
                        }
                        KeyCode::Char(c)
                            if (c.is_ascii_alphanumeric() || c == ' ')
                                && (app.current_tab == Tab::Search
                                    || !app.search_query.is_empty()) =>
                        {
                            app.search_query.push(c);
                            app.refresh_filter();
                        }
                        KeyCode::Backspace => {
                            app.search_query.pop();
                            app.refresh_filter();
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            let is_playing = app.playing.is_some();
            app.update_viz(is_playing);
            last_tick = Instant::now();
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(8),    // main content
            Constraint::Length(3), // status + help
        ])
        .split(f.area());

    // Header
    let header = Paragraph::new(Line::from(vec![
        Span::styled(" ♪ ", Style::default().fg(Color::Cyan)),
        Span::styled(
            "radio",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  —  various genres via Radio Browser"),
        Span::styled(
            "   [tab] switch  [d] cycle genre  [/] filter  [enter] play",
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    f.render_widget(header, chunks[0]);

    // Main area: list + now playing
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(chunks[1]);

    // Left: station list
    let filtered = app.filtered_stations();
    let items: Vec<ListItem> = filtered
        .iter()
        .map(|s| {
            let tag_preview = s
                .tags
                .split(',')
                .take(3)
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("·");
            let line = Line::from(vec![
                Span::styled(&s.name, Style::default().fg(Color::White)),
                Span::raw(" "),
                Span::styled(
                    format!("({})", s.country),
                    Style::default().fg(Color::Yellow),
                ),
                if !tag_preview.is_empty() {
                    Span::styled(
                        format!("  {}", tag_preview),
                        Style::default().fg(Color::DarkGray),
                    )
                } else {
                    Span::raw("")
                },
            ]);
            ListItem::new(line)
        })
        .collect();

    let title = match app.current_tag_filter {
        Some(i) => format!(" Stations — {} ", app.discover_tags[i]),
        None => " Stations ".to_string(),
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, main_chunks[0], &mut app.list_state);

    // Right: Now Playing + graphical visualizer
    let now_block = Block::default()
        .title(" Now Playing ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let now_area = now_block.inner(main_chunks[1]);
    f.render_widget(now_block, main_chunks[1]);

    let inner_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // station info
            Constraint::Min(6),    // visualizer
            Constraint::Length(2), // controls
        ])
        .split(now_area);

    if let Some(p) = &app.playing {
        let info = Paragraph::new(vec![
            Line::from(Span::styled(
                &p.station.name,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::raw("vol: "),
                Span::styled(format!("{}%", p.volume), Style::default().fg(Color::Cyan)),
                Span::raw("   "),
                Span::styled(&p.station.country, Style::default().fg(Color::Yellow)),
                Span::raw("  "),
                Span::styled(&p.station.codec, Style::default().fg(Color::Gray)),
            ]),
            Line::from(Span::styled(
                p.station
                    .tags
                    .split(',')
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" "),
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        f.render_widget(info, inner_layout[0]);

        // Graphical visualizer
        let bar_values: Vec<Bar> = app
            .viz_bars
            .iter()
            .enumerate()
            .map(|(i, &h)| {
                let color = if i % 3 == 0 {
                    Color::Cyan
                } else if i % 2 == 0 {
                    Color::Magenta
                } else {
                    Color::Blue
                };
                Bar::default()
                    .value(h as u64)
                    .label("".into())
                    .style(Style::default().fg(color))
            })
            .collect();

        let barchart = BarChart::default()
            .block(Block::default().title(" visualizer "))
            .data(BarGroup::default().bars(&bar_values))
            .bar_width(1)
            .bar_gap(0)
            .value_style(Style::default().fg(Color::DarkGray));

        f.render_widget(barchart, inner_layout[1]);
    } else {
        let idle = Paragraph::new(vec![
            Line::from(Span::styled(
                "No station playing",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Select a station and press Enter",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                "or press 'd' to browse genres",
                Style::default().fg(Color::Gray),
            )),
        ]);
        f.render_widget(idle, inner_layout[0]);
    }

    // Controls hint inside now playing
    let controls = Paragraph::new(Line::from(vec![Span::raw(
        "[enter] play  [s] stop  [+/-] vol  [d] genre  [q] quit",
    )]))
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(controls, inner_layout[2]);

    // Bottom status
    let filter_info = match (&app.search_query, app.current_tag_filter) {
        (q, Some(i)) if !q.is_empty() => format!("filter: {} + {}", app.discover_tags[i], q),
        (q, _) if !q.is_empty() => format!("filter: {}", q),
        (_, Some(i)) => format!("genre: {}", app.discover_tags[i]),
        _ => "all stations".to_string(),
    };

    let status_line = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(&app.status, Style::default().fg(Color::White)),
        Span::raw("   "),
        Span::styled(filter_info, Style::default().fg(Color::Blue)),
    ]);

    let status = Paragraph::new(status_line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(status, chunks[2]);
}
