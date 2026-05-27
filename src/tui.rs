use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Stdout, Write},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap},
};

use crate::installer::Tool;

type Tui = Terminal<CrosstermBackend<Stdout>>;
type PtyWriter = Box<dyn Write + Send>;

// ── palette ───────────────────────────────────────────────────────────────────
const CYAN: Color      = Color::Cyan;
const YELLOW: Color    = Color::Yellow;
const GREEN: Color     = Color::Green;
const RED: Color       = Color::Red;
const GRAY: Color      = Color::Gray;
const DARK: Color      = Color::DarkGray;
const ACCENT_BG: Color = Color::Rgb(20, 28, 48);

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const MAX_LOG_LINES: usize = 4000;

// ── messages from install threads ─────────────────────────────────────────────
enum Msg {
    Started(usize),
    Writer(usize, PtyWriter),
    Line(usize, String),
    /// In-progress line that hasn't seen a newline yet (e.g. sudo prompt).
    /// Replaces any previous partial for this tool.
    Partial(usize, String),
    Finished(usize, bool),
}

// ── tool status ───────────────────────────────────────────────────────────────
#[derive(Clone, PartialEq)]
enum ToolStatus {
    NotSelected,
    Pending,
    Running,
    Done,
    Failed,
}

// ── post-install prompt state ─────────────────────────────────────────────────
struct PostPrompt {
    tool: Tool,
    label: String,
    input: String,
    error: Option<String>,
}

// ── install state — persists across Selecting <-> Installing toggles ──────────
struct InstallState {
    statuses: Vec<ToolStatus>,
    tool_logs: Vec<Vec<String>>,
    partials: Vec<String>,
    writers: HashMap<usize, PtyWriter>,
    rx: mpsc::Receiver<Msg>,
    add_tx: mpsc::Sender<Vec<Tool>>,
    pending_prompts: Vec<Tool>,
    /// Cursor in the install-phase tool list (index into Tool::ALL).
    cursor: usize,
    list_state: ListState,
    attached: Option<usize>,
    scroll_offset: usize,
    /// Cached layout rects from the last render — used for mouse hit-testing.
    layout: InstallLayout,
    /// Per-tool: last partial-line text we auto-answered with the sudo password.
    /// Lets us re-answer when sudo re-prompts (different text) but not spam
    /// the same prompt repeatedly.
    last_pwd_prompt: HashMap<usize, String>,
}

#[derive(Default, Clone, Copy)]
struct InstallLayout {
    status_list: Rect,
    preview: Rect,
    attached_view: Rect,
}

// ── app phases ────────────────────────────────────────────────────────────────
enum Phase {
    Selecting,
    /// One-time popup asked when the user kicks off an install whose tools
    /// will invoke sudo. Captures a password kept in memory for the session
    /// so sudo prompts can be answered automatically by the TUI.
    SudoPrompt { input: String, error: Option<String>, pending: Vec<Tool> },
    Installing,
    PostInstall(PostPrompt),
    Done,
}

// ── app ───────────────────────────────────────────────────────────────────────
struct App {
    tools: &'static [Tool],
    cursor: usize,
    checked: Vec<bool>,
    list_state: ListState,
    /// Cached select-list rect for mouse hit-testing.
    select_list_rect: Rect,
    install: Option<InstallState>,
    phase: Phase,
    tick: u64,
    /// Sudo password captured once by the user; used to auto-answer sudo
    /// prompts on any PTY. None = user skipped the prompt.
    sudo_password: Option<String>,
}

impl App {
    fn new() -> Self {
        let tools = Tool::ALL;
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            tools,
            cursor: 0,
            checked: vec![false; tools.len()],
            list_state,
            select_list_rect: Rect::default(),
            install: None,
            phase: Phase::Selecting,
            tick: 0,
            sudo_password: None,
        }
    }

    fn chosen(&self) -> Vec<Tool> {
        self.tools.iter().zip(&self.checked)
            .filter_map(|(&t, &c)| c.then_some(t))
            .collect()
    }

    fn checked_count(&self) -> usize {
        self.checked.iter().filter(|&&c| c).count()
    }
}

// ── public entry point ────────────────────────────────────────────────────────
pub fn run_installer() -> Result<()> {
    let mut terminal = init()?;
    let mut app = App::new();
    let result = run_loop(&mut terminal, &mut app);
    restore(&mut terminal)?;
    result
}

fn init() -> Result<Tui> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(io::stdout()))?)
}

fn restore(t: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(t.backend_mut(), DisableMouseCapture, LeaveAlternateScreen)?;
    t.show_cursor()?;
    Ok(())
}

// ── main event loop ───────────────────────────────────────────────────────────
fn run_loop(terminal: &mut Tui, app: &mut App) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| render(f, app))?;

        if last_tick.elapsed() >= Duration::from_millis(80) {
            app.tick = app.tick.wrapping_add(1);
            last_tick = Instant::now();
        }

        drain_messages(app);
        check_prompts(app);

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if handle_key(app, key.code, key.modifiers)? {
                        return Ok(());
                    }
                }
                Event::Mouse(m) => handle_mouse(app, m),
                Event::Resize(_, _) => { /* re-render next frame */ }
                _ => {}
            }
        }

        if matches!(app.phase, Phase::Done) {
            return Ok(());
        }
    }
}

fn drain_messages(app: &mut App) {
    let pwd = app.sudo_password.clone();
    let Some(inst) = app.install.as_mut() else { return };
    loop {
        match inst.rx.try_recv() {
            Ok(Msg::Started(i))     => { inst.statuses[i] = ToolStatus::Running; }
            Ok(Msg::Writer(i, w))   => { inst.writers.insert(i, w); }
            Ok(Msg::Line(i, text))  => {
                inst.partials[i].clear();
                inst.last_pwd_prompt.remove(&i);
                let buf = &mut inst.tool_logs[i];
                if buf.len() >= MAX_LOG_LINES { buf.remove(0); }
                buf.push(text);
            }
            Ok(Msg::Partial(i, text)) => {
                inst.partials[i] = text.clone();
                if let Some(pw) = &pwd {
                    if looks_like_sudo_prompt(&text)
                        && inst.last_pwd_prompt.get(&i) != Some(&text)
                    {
                        if let Some(w) = inst.writers.get_mut(&i) {
                            let _ = w.write_all(pw.as_bytes());
                            let _ = w.write_all(b"\n");
                            let _ = w.flush();
                            inst.last_pwd_prompt.insert(i, text);
                        }
                    }
                }
            }
            Ok(Msg::Finished(i, ok)) => {
                inst.statuses[i] = if ok { ToolStatus::Done } else { ToolStatus::Failed };
                inst.writers.remove(&i);
                inst.last_pwd_prompt.remove(&i);
                if !inst.partials[i].is_empty() {
                    let t = std::mem::take(&mut inst.partials[i]);
                    inst.tool_logs[i].push(t);
                }
            }
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
        }
    }
}

/// Heuristic detection of a sudo / password prompt line.
fn looks_like_sudo_prompt(line: &str) -> bool {
    let trim = line.trim_end();
    if !trim.ends_with(':') { return false; }
    let lower = trim.to_lowercase();
    lower.contains("[sudo]")
        || lower.contains("password for")
        || lower.contains("password:")
        || lower.contains("contraseña")
}

fn is_all_done(inst: &InstallState) -> bool {
    inst.statuses.iter().all(|s| !matches!(s, ToolStatus::Pending | ToolStatus::Running))
        && inst.statuses.iter().any(|s| *s != ToolStatus::NotSelected)
}

fn check_prompts(app: &mut App) {
    let needs_prompt = match (&app.phase, &app.install) {
        (Phase::Installing, Some(inst)) => {
            is_all_done(inst) && !inst.pending_prompts.is_empty() && inst.attached.is_none()
        }
        _ => false,
    };
    if !needs_prompt { return; }

    let inst = app.install.as_mut().unwrap();
    let pos = inst.pending_prompts.iter().position(|tool| {
        let idx = Tool::ALL.iter().position(|&t| t == *tool).unwrap();
        inst.statuses[idx] == ToolStatus::Done
    });

    match pos {
        Some(i) => {
            let tool = inst.pending_prompts.remove(i);
            app.phase = Phase::PostInstall(make_prompt(tool));
        }
        None => {
            inst.pending_prompts.clear();
            // No more prompts; if everything's done, transition to Done. Otherwise idle.
            // We stay in Installing so the user can review the output.
        }
    }
}

fn make_prompt(tool: Tool) -> PostPrompt {
    match tool {
        Tool::Postgres => PostPrompt {
            tool,
            label: "Set password for postgres superuser".to_string(),
            input: String::new(),
            error: None,
        },
        _ => unreachable!("no prompt for this tool"),
    }
}

// ── key handling ──────────────────────────────────────────────────────────────
fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    match &app.phase {
        Phase::Selecting        => handle_select_key(app, code),
        Phase::SudoPrompt { .. } => handle_sudo_key(app, code),
        Phase::Installing       => handle_install_key(app, code, mods),
        Phase::PostInstall(_)   => handle_post_key(app, code),
        Phase::Done             => Ok(true),
    }
}

fn handle_sudo_key(app: &mut App, code: KeyCode) -> Result<bool> {
    let Phase::SudoPrompt { ref mut input, ref mut error, .. } = app.phase else { return Ok(false) };
    let _ = error;
    match code {
        KeyCode::Char(c)   => { input.push(c); }
        KeyCode::Backspace => { input.pop(); }
        KeyCode::Esc => {
            // User skipped — proceed without auto-answer.
            if let Phase::SudoPrompt { pending, .. } = std::mem::replace(&mut app.phase, Phase::Selecting) {
                app.sudo_password = None;
                proceed_to_install(app, pending);
            }
        }
        KeyCode::Enter => {
            if let Phase::SudoPrompt { input, pending, .. } = std::mem::replace(&mut app.phase, Phase::Selecting) {
                app.sudo_password = if input.is_empty() { None } else { Some(input) };
                proceed_to_install(app, pending);
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_select_key(app: &mut App, code: KeyCode) -> Result<bool> {
    match code {
        KeyCode::Char('q') => {
            // 'q' quits even if installs are running (they'll be terminated on exit).
            return Ok(true);
        }
        KeyCode::Esc => {
            // If an install is in progress, esc goes back to it instead of quitting.
            if app.install.is_some() {
                app.phase = Phase::Installing;
            } else {
                return Ok(true);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.cursor > 0 {
                app.cursor -= 1;
                app.list_state.select(Some(app.cursor));
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.cursor < app.tools.len() - 1 {
                app.cursor += 1;
                app.list_state.select(Some(app.cursor));
            }
        }
        KeyCode::Char(' ') => {
            toggle_checked(app, app.cursor);
        }
        KeyCode::Char('a') => {
            let all = app.checked.iter().all(|&c| c);
            app.checked.iter_mut().for_each(|c| *c = !all);
        }
        KeyCode::Enter => submit_selection(app),
        _ => {}
    }
    Ok(false)
}

fn toggle_checked(app: &mut App, idx: usize) {
    // Don't allow toggling tools currently running or already completed.
    if let Some(inst) = &app.install {
        if matches!(inst.statuses[idx], ToolStatus::Pending | ToolStatus::Running | ToolStatus::Done) {
            return;
        }
    }
    app.checked[idx] = !app.checked[idx];
}

fn submit_selection(app: &mut App) {
    let chosen = app.chosen();
    if chosen.is_empty() && app.install.is_some() {
        app.phase = Phase::Installing;
        return;
    }
    if chosen.is_empty() { return; }

    // If we don't have a password cached yet and any chosen tool will need
    // sudo, show the one-time prompt before starting.
    let needs_sudo = chosen.iter().any(|t| t.needs_pacman_lock());
    if needs_sudo && app.sudo_password.is_none() {
        app.phase = Phase::SudoPrompt {
            input: String::new(),
            error: None,
            pending: chosen,
        };
        return;
    }

    proceed_to_install(app, chosen);
}

fn proceed_to_install(app: &mut App, chosen: Vec<Tool>) {
    if app.install.is_none() {
        begin_install(app, chosen);
    } else {
        add_to_install(app, chosen);
    }
    for c in app.checked.iter_mut() { *c = false; }
    app.phase = Phase::Installing;
}

fn handle_install_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    let Some(inst) = app.install.as_mut() else { return Ok(false) };

    let visible: Vec<usize> = Tool::ALL.iter().enumerate()
        .filter(|(i, _)| inst.statuses[*i] != ToolStatus::NotSelected)
        .map(|(i, _)| i)
        .collect();
    if visible.is_empty() { return Ok(false); }

    // ── attached mode: forward keys to the tool's PTY ─────────────────────────
    if let Some(idx) = inst.attached {
        match code {
            KeyCode::Esc => { inst.attached = None; inst.scroll_offset = 0; }
            KeyCode::PageUp   => { inst.scroll_offset = inst.scroll_offset.saturating_add(10); }
            KeyCode::PageDown => { inst.scroll_offset = inst.scroll_offset.saturating_sub(10); }
            _ => {
                if let Some(w) = inst.writers.get_mut(&idx) {
                    let _ = write_key_to_pty(w.as_mut(), code, mods);
                }
            }
        }
        return Ok(false);
    }

    // ── list mode ─────────────────────────────────────────────────────────────
    // When the highlighted tool is running, most keys are forwarded to its PTY
    // so the user can answer prompts inline in the right panel. Only a small
    // set of keys stays reserved for the app.
    let pos_in_visible = visible.iter().position(|&i| i == inst.cursor).unwrap_or(0);
    let all_done = is_all_done(inst);
    let has_writer = inst.writers.contains_key(&inst.cursor);

    // Always-reserved app keys
    match code {
        KeyCode::F(2) => {
            inst.attached = Some(inst.cursor);
            inst.scroll_offset = 0;
            return Ok(false);
        }
        KeyCode::Char('f') if mods.contains(KeyModifiers::CONTROL) => {
            inst.attached = Some(inst.cursor);
            inst.scroll_offset = 0;
            return Ok(false);
        }
        KeyCode::Up => {
            if pos_in_visible > 0 {
                inst.cursor = visible[pos_in_visible - 1];
                inst.list_state.select(Some(pos_in_visible - 1));
            }
            return Ok(false);
        }
        KeyCode::Down => {
            if pos_in_visible + 1 < visible.len() {
                inst.cursor = visible[pos_in_visible + 1];
                inst.list_state.select(Some(pos_in_visible + 1));
            }
            return Ok(false);
        }
        KeyCode::PageUp   => { inst.scroll_offset = inst.scroll_offset.saturating_add(5); return Ok(false); }
        KeyCode::PageDown => { inst.scroll_offset = inst.scroll_offset.saturating_sub(5); return Ok(false); }
        KeyCode::Tab => {
            app.phase = Phase::Selecting;
            return Ok(false);
        }
        KeyCode::Esc => {
            // If something's still running, esc just goes to selection screen;
            // when everything's done and idle it quits.
            if all_done && inst.writers.is_empty() {
                return Ok(true);
            }
            app.phase = Phase::Selecting;
            return Ok(false);
        }
        _ => {}
    }

    if has_writer {
        // Forward to PTY of the highlighted running tool.
        if let Some(w) = inst.writers.get_mut(&inst.cursor) {
            let _ = write_key_to_pty(w.as_mut(), code, mods);
        }
        return Ok(false);
    }

    // No writer (tool done/failed/pending): app shortcuts on letters/Enter.
    match code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('j') => {
            if pos_in_visible + 1 < visible.len() {
                inst.cursor = visible[pos_in_visible + 1];
                inst.list_state.select(Some(pos_in_visible + 1));
            }
        }
        KeyCode::Char('k') => {
            if pos_in_visible > 0 {
                inst.cursor = visible[pos_in_visible - 1];
                inst.list_state.select(Some(pos_in_visible - 1));
            }
        }
        KeyCode::Enter => {
            inst.attached = Some(inst.cursor);
            inst.scroll_offset = 0;
        }
        _ => {}
    }
    Ok(false)
}

/// Translate a crossterm key event into bytes a TTY expects.
fn write_key_to_pty(w: &mut dyn Write, code: KeyCode, mods: KeyModifiers) -> io::Result<()> {
    let bytes: Vec<u8> = match code {
        KeyCode::Char(c) => {
            if mods.contains(KeyModifiers::CONTROL) {
                let lc = c.to_ascii_lowercase();
                if ('a'..='z').contains(&lc) {
                    vec![(lc as u8) - b'a' + 1]
                } else {
                    let mut s = String::new();
                    s.push(c);
                    s.into_bytes()
                }
            } else {
                let mut s = String::new();
                s.push(c);
                s.into_bytes()
            }
        }
        KeyCode::Enter      => vec![b'\r'],
        KeyCode::Tab        => vec![b'\t'],
        KeyCode::Backspace  => vec![0x7f],
        KeyCode::Delete     => b"\x1b[3~".to_vec(),
        KeyCode::Up         => b"\x1b[A".to_vec(),
        KeyCode::Down       => b"\x1b[B".to_vec(),
        KeyCode::Right      => b"\x1b[C".to_vec(),
        KeyCode::Left       => b"\x1b[D".to_vec(),
        KeyCode::Home       => b"\x1b[H".to_vec(),
        KeyCode::End        => b"\x1b[F".to_vec(),
        _ => return Ok(()),
    };
    w.write_all(&bytes)?;
    w.flush()
}

fn handle_post_key(app: &mut App, code: KeyCode) -> Result<bool> {
    let Phase::PostInstall(ref mut prompt) = app.phase else { return Ok(false) };

    match code {
        KeyCode::Char(c)   => { prompt.input.push(c); }
        KeyCode::Backspace => { prompt.input.pop(); }
        KeyCode::Esc       => { app.phase = Phase::Done; }
        KeyCode::Enter => {
            let password = prompt.input.clone();
            let tool = prompt.tool;
            match run_post_install(tool, &password) {
                Ok(()) => { app.phase = Phase::Done; }
                Err(e) => {
                    if let Phase::PostInstall(ref mut p) = app.phase {
                        p.error = Some(e.to_string());
                    }
                }
            }
        }
        _ => {}
    }
    Ok(false)
}

fn run_post_install(tool: Tool, value: &str) -> Result<()> {
    match tool {
        Tool::Postgres => crate::installer::postgres::set_password(value),
        _ => Ok(()),
    }
}

// ── mouse handling ────────────────────────────────────────────────────────────
fn handle_mouse(app: &mut App, m: MouseEvent) {
    let (col, row) = (m.column, m.row);

    match (&app.phase, m.kind) {
        // ── Selecting phase ───────────────────────────────────────────────────
        (Phase::Selecting, MouseEventKind::Down(MouseButton::Left)) => {
            if point_in(col, row, app.select_list_rect) {
                if let Some(i) = row_in_list(row, app.select_list_rect, app.tools.len()) {
                    app.cursor = i;
                    app.list_state.select(Some(i));
                }
            }
        }
        (Phase::Selecting, MouseEventKind::ScrollUp) => {
            if app.cursor > 0 {
                app.cursor -= 1;
                app.list_state.select(Some(app.cursor));
            }
        }
        (Phase::Selecting, MouseEventKind::ScrollDown) => {
            if app.cursor + 1 < app.tools.len() {
                app.cursor += 1;
                app.list_state.select(Some(app.cursor));
            }
        }

        // ── Installing phase ──────────────────────────────────────────────────
        (Phase::Installing, kind) => {
            let Some(inst) = app.install.as_mut() else { return };
            if let Some(att) = inst.attached {
                match kind {
                    MouseEventKind::ScrollUp   => { inst.scroll_offset = inst.scroll_offset.saturating_add(3); }
                    MouseEventKind::ScrollDown => { inst.scroll_offset = inst.scroll_offset.saturating_sub(3); }
                    _ => {}
                }
                let _ = att;
                return;
            }

            let visible: Vec<usize> = Tool::ALL.iter().enumerate()
                .filter(|(i, _)| inst.statuses[*i] != ToolStatus::NotSelected)
                .map(|(i, _)| i)
                .collect();
            if visible.is_empty() { return; }

            match kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if point_in(col, row, inst.layout.status_list) {
                        if let Some(i) = row_in_list(row, inst.layout.status_list, visible.len()) {
                            inst.cursor = visible[i];
                            inst.list_state.select(Some(i));
                        }
                    }
                    // Clicks on the preview panel are intentionally a no-op:
                    // the highlighted tool already receives all typed keys.
                }
                MouseEventKind::ScrollUp => {
                    if point_in(col, row, inst.layout.preview) {
                        inst.scroll_offset = inst.scroll_offset.saturating_add(3);
                    } else {
                        let pos = visible.iter().position(|&i| i == inst.cursor).unwrap_or(0);
                        if pos > 0 {
                            inst.cursor = visible[pos - 1];
                            inst.list_state.select(Some(pos - 1));
                        }
                    }
                }
                MouseEventKind::ScrollDown => {
                    if point_in(col, row, inst.layout.preview) {
                        inst.scroll_offset = inst.scroll_offset.saturating_sub(3);
                    } else {
                        let pos = visible.iter().position(|&i| i == inst.cursor).unwrap_or(0);
                        if pos + 1 < visible.len() {
                            inst.cursor = visible[pos + 1];
                            inst.list_state.select(Some(pos + 1));
                        }
                    }
                }
                _ => {}
            }
        }

        _ => {}
    }
}

fn point_in(col: u16, row: u16, r: Rect) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// Given a row clicked inside `area` rendering a List (with our standard
/// 1-row top padding), return which item index was hit.
fn row_in_list(row: u16, area: Rect, item_count: usize) -> Option<usize> {
    // Our list blocks use `Padding::new(_, _, 1, 0)` (1 row top padding).
    let top = area.y.saturating_add(1);
    if row < top { return None; }
    let i = (row - top) as usize;
    if i < item_count { Some(i) } else { None }
}

// ── install orchestration ─────────────────────────────────────────────────────
fn begin_install(app: &mut App, chosen: Vec<Tool>) {
    let mut statuses = vec![ToolStatus::NotSelected; Tool::ALL.len()];
    let mut pending_prompts: Vec<Tool> = chosen.iter()
        .filter(|t| t.has_post_install())
        .copied()
        .collect();
    pending_prompts.sort_by_key(|t| Tool::ALL.iter().position(|&x| x == *t).unwrap());

    for &tool in &chosen {
        let idx = Tool::ALL.iter().position(|&t| t == tool).unwrap();
        statuses[idx] = ToolStatus::Pending;
    }

    let tool_logs: Vec<Vec<String>> = (0..Tool::ALL.len()).map(|_| Vec::new()).collect();
    let partials:  Vec<String>      = (0..Tool::ALL.len()).map(|_| String::new()).collect();

    let first_selected = statuses.iter().position(|s| *s != ToolStatus::NotSelected).unwrap_or(0);
    let mut list_state = ListState::default();
    list_state.select(Some(0));

    let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
    let (add_tx, add_rx) = mpsc::channel::<Vec<Tool>>();
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "custom-tools".to_string());

    spawn_scheduler(chosen, exe, add_rx, msg_tx);

    app.install = Some(InstallState {
        statuses,
        tool_logs,
        partials,
        writers: HashMap::new(),
        rx: msg_rx,
        add_tx,
        pending_prompts,
        cursor: first_selected,
        list_state,
        attached: None,
        scroll_offset: 0,
        layout: InstallLayout::default(),
        last_pwd_prompt: HashMap::new(),
    });
}

fn add_to_install(app: &mut App, chosen: Vec<Tool>) {
    let Some(inst) = app.install.as_mut() else { return };

    let mut to_add = Vec::new();
    for tool in chosen {
        let idx = Tool::ALL.iter().position(|&t| t == tool).unwrap();
        // Skip tools already in flight or done.
        match inst.statuses[idx] {
            ToolStatus::Pending | ToolStatus::Running | ToolStatus::Done => continue,
            _ => {}
        }
        inst.statuses[idx] = ToolStatus::Pending;
        if tool.has_post_install() && !inst.pending_prompts.contains(&tool) {
            inst.pending_prompts.push(tool);
        }
        to_add.push(tool);
    }
    if !to_add.is_empty() {
        let _ = inst.add_tx.send(to_add);
    }
}

/// Long-running scheduler. Maintains queue/running/completed; spawns tools
/// whose deps are met, respecting the pacman lock (at most one pacman tool
/// running at a time).
fn spawn_scheduler(
    initial: Vec<Tool>,
    exe: String,
    add_rx: mpsc::Receiver<Vec<Tool>>,
    tui_tx: mpsc::Sender<Msg>,
) {
    thread::spawn(move || {
        let mut completed: HashSet<Tool> = Tool::ALL.iter()
            .copied()
            .filter(|t| t.is_installed())
            .collect();
        let mut running: HashSet<Tool> = HashSet::new();
        let mut queue:   Vec<Tool>     = initial;
        let (done_tx, done_rx) = mpsc::channel::<Tool>();
        let mut add_open = true;

        loop {
            // Drain incoming "add more tools" messages.
            if add_open {
                loop {
                    match add_rx.try_recv() {
                        Ok(more) => {
                            for t in more {
                                if !completed.contains(&t)
                                    && !running.contains(&t)
                                    && !queue.contains(&t)
                                {
                                    queue.push(t);
                                }
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => { add_open = false; break; }
                    }
                }
            }

            // Drain completed tools.
            loop {
                match done_rx.try_recv() {
                    Ok(t) => { running.remove(&t); completed.insert(t); }
                    Err(_) => break,
                }
            }

            // Spawn anything that's ready.
            let mut pacman_busy = running.iter().any(|t| t.needs_pacman_lock());
            let mut i = 0;
            while i < queue.len() {
                let t = queue[i];
                let deps_ok = t.deps().iter().all(|d| completed.contains(d));
                let pacman_ok = !t.needs_pacman_lock() || !pacman_busy;
                if deps_ok && pacman_ok {
                    queue.remove(i);
                    running.insert(t);
                    if t.needs_pacman_lock() { pacman_busy = true; }
                    let idx = Tool::ALL.iter().position(|&x| x == t).unwrap();
                    let arg = t.arg_name();
                    let tx_msg  = tui_tx.clone();
                    let tx_done = done_tx.clone();
                    let exe2 = exe.clone();
                    thread::spawn(move || {
                        run_in_pty(idx, arg, &exe2, tx_msg);
                        let _ = tx_done.send(t);
                    });
                } else {
                    i += 1;
                }
            }

            // If no more work can come in and nothing is left, exit.
            if !add_open && queue.is_empty() && running.is_empty() {
                return;
            }

            thread::sleep(Duration::from_millis(60));
        }
    });
}

/// Spawn `<exe> install <bin_name>` inside a fresh PTY.
fn run_in_pty(tool_idx: usize, bin_name: &str, exe: &str, tx: mpsc::Sender<Msg>) {
    tx.send(Msg::Started(tool_idx)).ok();

    let pty_system = NativePtySystem::default();
    let pair = match pty_system.openpty(PtySize {
        rows: 40,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            tx.send(Msg::Line(tool_idx, format!("error opening pty: {e}"))).ok();
            tx.send(Msg::Finished(tool_idx, false)).ok();
            return;
        }
    };

    let mut cmd = CommandBuilder::new(exe);
    cmd.arg("install");
    cmd.arg(bin_name);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }
    for (k, v) in std::env::vars() {
        cmd.env(k, v);
    }

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            tx.send(Msg::Line(tool_idx, format!("error spawning: {e}"))).ok();
            tx.send(Msg::Finished(tool_idx, false)).ok();
            return;
        }
    };

    drop(pair.slave);

    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            tx.send(Msg::Line(tool_idx, format!("error cloning reader: {e}"))).ok();
            let _ = child.kill();
            tx.send(Msg::Finished(tool_idx, false)).ok();
            return;
        }
    };

    match pair.master.take_writer() {
        Ok(w) => { tx.send(Msg::Writer(tool_idx, w)).ok(); }
        Err(e) => {
            tx.send(Msg::Line(tool_idx, format!("error taking writer: {e}"))).ok();
        }
    }

    let tx_r = tx.clone();
    let reader_thread = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut current = String::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let s = String::from_utf8_lossy(&buf[..n]);
                    for ch in s.chars() {
                        match ch {
                            '\n' => {
                                let line = std::mem::take(&mut current);
                                let _ = tx_r.send(Msg::Line(tool_idx, strip_ansi(&line)));
                            }
                            '\r' => {
                                if !current.is_empty() {
                                    let line = std::mem::take(&mut current);
                                    let _ = tx_r.send(Msg::Line(tool_idx, strip_ansi(&line)));
                                }
                            }
                            _ => current.push(ch),
                        }
                    }
                    // Flush whatever is buffered without a newline so the TUI
                    // can show prompts like "[sudo] password for jostickq:".
                    if !current.is_empty() {
                        let _ = tx_r.send(Msg::Partial(tool_idx, strip_ansi(&current)));
                    }
                }
                Err(_) => break,
            }
        }
        if !current.is_empty() {
            let _ = tx_r.send(Msg::Line(tool_idx, strip_ansi(&current)));
        }
    });

    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    reader_thread.join().ok();
    drop(pair.master);
    tx.send(Msg::Finished(tool_idx, ok)).ok();
}

/// Strip ANSI CSI/OSC escape sequences and other C0 control chars.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    while let Some(n) = chars.next() {
                        if n.is_ascii_alphabetic() || n == '~' { break; }
                    }
                }
                Some(']') => {
                    while let Some(n) = chars.next() {
                        if n == '\x07' { break; }
                        if n == '\x1b' { chars.next(); break; }
                    }
                }
                _ => {}
            }
        } else if (c as u32) >= 0x20 || c == '\t' {
            out.push(c);
        }
    }
    out
}

// ── rendering ─────────────────────────────────────────────────────────────────
fn render(frame: &mut ratatui::Frame, app: &mut App) {
    match &app.phase {
        Phase::Selecting        => render_select(frame, app),
        Phase::SudoPrompt { .. } => { render_select(frame, app); render_sudo(frame, app); }
        Phase::Installing       => render_install(frame, app),
        Phase::PostInstall(_)   => render_post(frame, app),
        Phase::Done             => {}
    }
}

fn render_sudo(frame: &mut ratatui::Frame, app: &App) {
    let Phase::SudoPrompt { input, error, .. } = &app.phase else { return };
    let area = frame.area();

    let popup_width  = 68u16.min(area.width.saturating_sub(4));
    let popup_height = 14u16;
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    let block = Block::default()
        .title(Span::styled(
            " Sudo password (optional) ",
            Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(CYAN));

    let masked = "•".repeat(input.len());

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Enter once; we'll auto-fill it whenever sudo prompts",
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  during this session. Kept in memory only.",
            Style::default().fg(GRAY),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Password: ", Style::default().fg(GRAY)),
            Span::styled(masked, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("▋", Style::default().fg(CYAN).add_modifier(Modifier::SLOW_BLINK)),
        ]),
        Line::from(""),
        if let Some(err) = error {
            Line::from(Span::styled(format!("  ✗  {err}"), Style::default().fg(RED)))
        } else {
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled("enter", Style::default().fg(GREEN)),
                Span::styled("  use this password   ", Style::default().fg(DARK)),
                Span::styled("esc", Style::default().fg(RED)),
                Span::styled("  skip (answer manually) ", Style::default().fg(DARK)),
            ])
        },
    ];

    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(Color::Rgb(8, 8, 18))),
        area,
    );
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(block),
        popup_area,
    );
}

// ── select phase ──────────────────────────────────────────────────────────────
fn render_select(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.area();

    let outer = Block::default()
        .title(Line::from(vec![
            Span::raw("  "),
            Span::styled("⚙  Custom Tools Installer", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
        ]))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(CYAN));

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let vert  = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
    let horiz = Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)]).split(vert[0]);

    app.select_list_rect = horiz[0];

    render_select_list(frame, app, horiz[0]);
    render_select_detail(frame, app, horiz[1]);
    render_select_footer(frame, app, vert[1]);
}

fn render_select_list(frame: &mut ratatui::Frame, app: &mut App, area: Rect) {
    let n = app.checked_count();
    let title = Line::from(vec![
        Span::styled(" Tools ", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)),
        Span::styled(format!("({n} selected) "), Style::default().fg(DARK)),
    ]);

    let block = Block::default()
        .title(title)
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(DARK))
        .padding(Padding::new(1, 1, 1, 0));

    let items: Vec<ListItem> = app.tools.iter().enumerate().map(|(i, tool)| {
        let checked   = app.checked[i];
        let installed = tool.is_installed();

        // Status from in-progress install (if any) takes priority over "installed".
        let install_status = app.install.as_ref().map(|inst| inst.statuses[i].clone());

        let checkbox = match &install_status {
            Some(ToolStatus::Pending)  => Span::styled("[…] ", Style::default().fg(DARK)),
            Some(ToolStatus::Running)  => Span::styled("[~] ", Style::default().fg(YELLOW)),
            Some(ToolStatus::Done)     => Span::styled("[✓] ", Style::default().fg(GREEN)),
            Some(ToolStatus::Failed)   => Span::styled("[✗] ", Style::default().fg(RED)),
            _ => {
                if checked {
                    Span::styled("[✓] ", Style::default().fg(GREEN).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled("[ ] ", Style::default().fg(DARK))
                }
            }
        };

        let name_color = match &install_status {
            Some(ToolStatus::Pending | ToolStatus::Running) => DARK,
            Some(ToolStatus::Done) => GREEN,
            Some(ToolStatus::Failed) => RED,
            _ => if checked { GREEN } else { GRAY },
        };
        let name = Span::styled(
            format!("{:<10}", tool.bin_name()),
            Style::default().fg(name_color),
        );

        let dot = if installed && install_status.is_none() {
            Span::styled(" ●", Style::default().fg(GREEN))
        } else {
            Span::raw("  ")
        };

        ListItem::new(Line::from(vec![checkbox, name, dot]))
    }).collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(ACCENT_BG).fg(Color::White).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn render_select_detail(frame: &mut ratatui::Frame, app: &App, area: Rect) {
    let tool      = app.tools[app.cursor];
    let installed = tool.is_installed();
    let (title, desc, hint) = tool_detail(tool);

    let status = if installed {
        Line::from(vec![
            Span::styled("● ", Style::default().fg(GREEN)),
            Span::styled("already installed", Style::default().fg(GREEN)),
        ])
    } else {
        Line::from(vec![
            Span::styled("○ ", Style::default().fg(RED)),
            Span::styled("not installed", Style::default().fg(RED)),
        ])
    };

    let mut lines = vec![
        Line::from(Span::styled(title, Style::default().fg(CYAN).add_modifier(Modifier::BOLD))),
        Line::from(""),
    ];
    for word_line in textwrap(desc, 36) {
        lines.push(Line::from(Span::styled(word_line, Style::default().fg(GRAY))));
    }
    lines.push(Line::from(""));
    lines.push(status);
    if let Some(h) = hint {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!("⚠  {h}"), Style::default().fg(YELLOW))));
    }

    let para = Paragraph::new(Text::from(lines))
        .block(Block::default()
            .title(Span::styled(" Details ", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)))
            .borders(Borders::NONE)
            .padding(Padding::new(2, 1, 1, 0)))
        .wrap(Wrap { trim: false });

    frame.render_widget(para, area);
}

fn render_select_footer(frame: &mut ratatui::Frame, app: &App, area: Rect) {
    let n = app.checked_count();
    let install_label = if n == 0 { " install ".to_string() } else { format!(" install ({n}) ") };

    let mut spans = vec![
        Span::styled(" ↑↓ / jk", Style::default().fg(CYAN)),
        Span::styled("  navigate   ", Style::default().fg(DARK)),
        Span::styled("space", Style::default().fg(CYAN)),
        Span::styled("  toggle   ", Style::default().fg(DARK)),
        Span::styled("a", Style::default().fg(CYAN)),
        Span::styled("  all   ", Style::default().fg(DARK)),
        Span::styled("enter", Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
        Span::styled(install_label, Style::default().fg(GREEN)),
    ];
    if app.install.is_some() {
        spans.push(Span::styled("esc", Style::default().fg(CYAN)));
        spans.push(Span::styled(" back to installs   ", Style::default().fg(DARK)));
    }
    spans.push(Span::styled("q", Style::default().fg(RED)));
    spans.push(Span::styled("  quit ", Style::default().fg(DARK)));

    frame.render_widget(Paragraph::new(Line::from(spans)).alignment(Alignment::Center), area);
}

// ── install phase ─────────────────────────────────────────────────────────────
fn render_install(frame: &mut ratatui::Frame, app: &mut App) {
    let attached = app.install.as_ref().and_then(|i| i.attached);

    if let Some(idx) = attached {
        render_install_attached(frame, app, idx);
    } else {
        render_install_list(frame, app);
    }
}

fn render_install_list(frame: &mut ratatui::Frame, app: &mut App) {
    let tick = app.tick;
    let Some(inst) = app.install.as_mut() else { return };
    let area = frame.area();
    let all_done = is_all_done(inst);

    let (title, border_color) = if all_done {
        (" ✓  Done — enter to inspect, a to add more, q to exit ", GREEN)
    } else {
        (" ⏳  Installing — enter to attach, a to add more tools ", YELLOW)
    };

    let outer = Block::default()
        .title(Line::from(Span::styled(title, Style::default().fg(border_color).add_modifier(Modifier::BOLD))))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let vert  = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
    let horiz = Layout::horizontal([Constraint::Percentage(32), Constraint::Percentage(68)]).split(vert[0]);

    inst.layout.status_list   = horiz[0];
    inst.layout.preview       = horiz[1];
    inst.layout.attached_view = Rect::default();

    render_install_status_list(frame, &inst.statuses, tick, &mut inst.list_state, horiz[0]);
    render_install_preview(frame, inst.cursor, &inst.tool_logs[inst.cursor], &inst.partials[inst.cursor], &inst.statuses, inst.scroll_offset, horiz[1]);
    render_install_footer(frame, &inst.statuses, all_done, inst.writers.contains_key(&inst.cursor), vert[1]);
}

fn render_install_status_list(
    frame: &mut ratatui::Frame,
    statuses: &[ToolStatus],
    tick: u64,
    list_state: &mut ListState,
    area: Rect,
) {
    let spin = SPINNER[(tick as usize) % SPINNER.len()];

    let items: Vec<ListItem> = Tool::ALL.iter().enumerate()
        .filter(|(i, _)| statuses[*i] != ToolStatus::NotSelected)
        .map(|(i, tool)| {
            let (icon, style) = match &statuses[i] {
                ToolStatus::Pending  => ("  ○  ".to_string(), Style::default().fg(DARK)),
                ToolStatus::Running  => (format!("  {spin}  "), Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)),
                ToolStatus::Done     => ("  ✓  ".to_string(), Style::default().fg(GREEN)),
                ToolStatus::Failed   => ("  ✗  ".to_string(), Style::default().fg(RED)),
                ToolStatus::NotSelected => unreachable!(),
            };
            let name = Span::styled(tool.bin_name(), style);
            ListItem::new(Line::from(vec![Span::styled(icon, style), name]))
        })
        .collect();

    let block = Block::default()
        .title(Span::styled(" Status ", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)))
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(DARK))
        .padding(Padding::new(0, 1, 1, 0));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(ACCENT_BG).fg(Color::White).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶");

    frame.render_stateful_widget(list, area, list_state);
}

fn render_install_preview(
    frame: &mut ratatui::Frame,
    cursor: usize,
    log: &[String],
    partial: &str,
    statuses: &[ToolStatus],
    scroll_offset: usize,
    area: Rect,
) {
    let tool_name = Tool::ALL[cursor].bin_name();
    let (status_label, is_running) = match &statuses[cursor] {
        ToolStatus::Pending  => (Span::styled("pending", Style::default().fg(DARK)),    false),
        ToolStatus::Running  => (Span::styled("running", Style::default().fg(YELLOW)),  true),
        ToolStatus::Done     => (Span::styled("done",    Style::default().fg(GREEN)),   false),
        ToolStatus::Failed   => (Span::styled("failed",  Style::default().fg(RED)),     false),
        ToolStatus::NotSelected => (Span::raw(""), false),
    };

    let hint = if is_running {
        " — type here, F2 to expand "
    } else {
        " — enter or F2 to expand "
    };

    let title = Line::from(vec![
        Span::styled(format!(" {tool_name} "), Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Span::styled("· ", Style::default().fg(DARK)),
        status_label,
        Span::styled(hint, Style::default().fg(DARK)),
    ]);

    let inner_height = area.height.saturating_sub(2) as usize;
    let mut combined: Vec<String> = log.iter().cloned().collect();
    if !partial.is_empty() { combined.push(partial.to_string()); }

    let total = combined.len();
    let offset = scroll_offset.min(total.saturating_sub(inner_height));
    let end = total.saturating_sub(offset);
    let start = end.saturating_sub(inner_height);

    let lines: Vec<Line> = if total == 0 {
        vec![Line::from(Span::styled(
            "  (no output yet — once it starts, output appears here)",
            Style::default().fg(DARK),
        ))]
    } else {
        combined[start..end].iter().enumerate().map(|(i, t)| {
            let abs = start + i;
            let is_partial_line = !partial.is_empty() && abs == combined.len() - 1;
            let style = if is_partial_line {
                Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(GRAY)
            };
            Line::from(Span::styled(t.clone(), style))
        }).collect()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::NONE)
        .padding(Padding::new(1, 1, 1, 0));

    frame.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

fn render_install_attached(frame: &mut ratatui::Frame, app: &mut App, idx: usize) {
    let Some(inst) = app.install.as_mut() else { return };
    let area = frame.area();
    let tool_name = Tool::ALL[idx].bin_name();

    let (status_text, status_color) = match &inst.statuses[idx] {
        ToolStatus::Pending  => ("pending", DARK),
        ToolStatus::Running  => ("running", YELLOW),
        ToolStatus::Done     => ("done",    GREEN),
        ToolStatus::Failed   => ("failed",  RED),
        ToolStatus::NotSelected => ("", GRAY),
    };

    let title = Line::from(vec![
        Span::styled(format!(" ⮞ {tool_name} "), Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Span::styled("· ", Style::default().fg(DARK)),
        Span::styled(status_text, Style::default().fg(status_color)),
        Span::styled("  ", Style::default()),
    ]);

    let outer = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(CYAN));

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let vert = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
    inst.layout.attached_view = vert[0];

    let log = &inst.tool_logs[idx];
    let partial = &inst.partials[idx];
    let total = log.len() + if partial.is_empty() { 0 } else { 1 };
    let inner_height = vert[0].height as usize;
    let offset = inst.scroll_offset.min(total.saturating_sub(inner_height));
    let end = total.saturating_sub(offset);
    let start = end.saturating_sub(inner_height);

    let mut combined: Vec<String> = log.iter().cloned().collect();
    if !partial.is_empty() { combined.push(partial.clone()); }

    let lines: Vec<Line> = if combined.is_empty() {
        vec![Line::from(Span::styled(
            "  (waiting for output...)",
            Style::default().fg(DARK),
        ))]
    } else {
        combined[start..end].iter().enumerate().map(|(i, t)| {
            let abs = start + i;
            let is_partial = !partial.is_empty() && abs == combined.len() - 1;
            let style = if is_partial {
                Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(GRAY)
            };
            Line::from(Span::styled(t.clone(), style))
        }).collect()
    };

    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::default().padding(Padding::new(1, 1, 0, 0))),
        vert[0],
    );

    let still_running = matches!(inst.statuses[idx], ToolStatus::Running | ToolStatus::Pending);
    let footer = if still_running {
        Line::from(vec![
            Span::styled(" type ", Style::default().fg(DARK)),
            Span::styled("anything", Style::default().fg(CYAN)),
            Span::styled(" — keys go to this process   ", Style::default().fg(DARK)),
            Span::styled("PgUp/PgDn", Style::default().fg(CYAN)),
            Span::styled(" or mouse wheel scroll   ", Style::default().fg(DARK)),
            Span::styled("Esc", Style::default().fg(RED)),
            Span::styled(" back to list ", Style::default().fg(DARK)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" PgUp/PgDn", Style::default().fg(CYAN)),
            Span::styled(" or mouse wheel scroll   ", Style::default().fg(DARK)),
            Span::styled("Esc", Style::default().fg(RED)),
            Span::styled(" back to list ", Style::default().fg(DARK)),
        ])
    };

    frame.render_widget(Paragraph::new(footer).alignment(Alignment::Center), vert[1]);
}

fn render_install_footer(
    frame: &mut ratatui::Frame,
    statuses: &[ToolStatus],
    all_done: bool,
    current_running: bool,
    area: Rect,
) {
    let done    = statuses.iter().filter(|s| **s == ToolStatus::Done).count();
    let failed  = statuses.iter().filter(|s| **s == ToolStatus::Failed).count();
    let running = statuses.iter().filter(|s| **s == ToolStatus::Running).count();
    let total   = statuses.iter().filter(|s| **s != ToolStatus::NotSelected).count();

    let mut spans = vec![
        Span::styled(format!(" {done}/{total} done"), Style::default().fg(GREEN)),
    ];
    if running > 0 { spans.push(Span::styled(format!("  {running} running"), Style::default().fg(YELLOW))); }
    if failed  > 0 { spans.push(Span::styled(format!("  {failed} failed"),  Style::default().fg(RED))); }

    spans.push(Span::styled("    ↑↓", Style::default().fg(CYAN)));
    spans.push(Span::styled(" navigate   ", Style::default().fg(DARK)));
    if current_running {
        spans.push(Span::styled("type", Style::default().fg(CYAN)));
        spans.push(Span::styled(" → sends to tool   ", Style::default().fg(DARK)));
        spans.push(Span::styled("F2", Style::default().fg(CYAN)));
        spans.push(Span::styled(" expand   ", Style::default().fg(DARK)));
    } else {
        spans.push(Span::styled("enter/F2", Style::default().fg(GREEN)));
        spans.push(Span::styled(" expand   ", Style::default().fg(DARK)));
    }
    spans.push(Span::styled("Tab", Style::default().fg(CYAN)));
    spans.push(Span::styled(" add tools   ", Style::default().fg(DARK)));
    spans.push(Span::styled("PgUp/Dn", Style::default().fg(CYAN)));
    spans.push(Span::styled(" scroll", Style::default().fg(DARK)));

    if all_done {
        spans.push(Span::styled("    q", Style::default().fg(RED)));
        spans.push(Span::styled(" quit ", Style::default().fg(DARK)));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).alignment(Alignment::Center), area);
}

// ── post-install phase ────────────────────────────────────────────────────────
fn render_post(frame: &mut ratatui::Frame, app: &App) {
    let Phase::PostInstall(prompt) = &app.phase else { return };
    let area = frame.area();

    let popup_width  = 62u16.min(area.width.saturating_sub(4));
    let popup_height = 12u16;
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    let block = Block::default()
        .title(Span::styled(
            " Post-install Configuration ",
            Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(CYAN));

    let masked = "•".repeat(prompt.input.len());

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", prompt.label),
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Password: ", Style::default().fg(GRAY)),
            Span::styled(masked, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("▋", Style::default().fg(CYAN).add_modifier(Modifier::SLOW_BLINK)),
        ]),
        Line::from(""),
        if let Some(err) = &prompt.error {
            Line::from(Span::styled(
                format!("  ✗  {}", err.lines().next().unwrap_or(err)),
                Style::default().fg(RED),
            ))
        } else {
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled("enter", Style::default().fg(GREEN)),
                Span::styled("  confirm   ", Style::default().fg(DARK)),
                Span::styled("esc", Style::default().fg(RED)),
                Span::styled("  skip", Style::default().fg(DARK)),
            ])
        },
    ];

    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(Color::Rgb(8, 8, 18))),
        area,
    );
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(block),
        popup_area,
    );
}

// ── tool detail metadata ──────────────────────────────────────────────────────
fn tool_detail(tool: Tool) -> (&'static str, &'static str, Option<&'static str>) {
    match tool {
        Tool::Fish => (
            "fish — Friendly Interactive Shell",
            "Smart shell with syntax highlighting, autosuggestions, and sane defaults. Installed via pacman.",
            None,
        ),
        Tool::Rust => (
            "rust — Systems Toolchain",
            "Installs rustup with the stable toolchain (cargo, rustc, rustfmt, clippy). Required to build paru.",
            None,
        ),
        Tool::Starship => (
            "starship — Cross-Shell Prompt",
            "Minimal, fast prompt. Deploys your starship.toml config automatically.",
            None,
        ),
        Tool::Fnm => (
            "fnm — Fast Node Manager",
            "Rust-based Node version manager. Installs the latest stable Node release automatically.",
            None,
        ),
        Tool::Pnpm => (
            "pnpm — Efficient Package Manager",
            "Fast, disk-space efficient Node package manager. Uses a content-addressable store.",
            None,
        ),
        Tool::Xh => (
            "xh — Friendly HTTP Client",
            "Modern curl alternative written in Rust. Cleaner syntax, automatic JSON formatting.",
            None,
        ),
        Tool::Rg => (
            "rg — ripgrep",
            "Blazing-fast recursive search. Respects .gitignore, supports regex, written in Rust.",
            None,
        ),
        Tool::Fd => (
            "fd — Fast Find Alternative",
            "Intuitive, fast alternative to find. Colorized output, respects .gitignore.",
            None,
        ),
        Tool::Yay => (
            "yay — AUR Helper (Go)",
            "AUR helper written in Go. Required to install AUR packages like TablePlus.",
            None,
        ),
        Tool::Paru => (
            "paru — AUR Helper (Rust)",
            "Feature-rich AUR helper written in Rust. Needs Rust/cargo installed first.",
            Some("install rust first"),
        ),
        Tool::Postgres => (
            "postgresql — Database Server",
            "Latest PostgreSQL via pacman. Initializes cluster, enables service, prompts for superuser password.",
            None,
        ),
        Tool::Docker => (
            "docker — Container Runtime",
            "docker + docker-compose via pacman, enables the service, adds you to the docker group.",
            None,
        ),
        Tool::TablePlus => (
            "tableplus — Database GUI",
            "Native GUI for Postgres, MySQL, SQLite and more. Installed from AUR.",
            Some("needs yay or paru installed"),
        ),
        Tool::Zoxide => (
            "zoxide — Smarter cd",
            "Learns your most-visited directories and lets you jump instantly. Drop-in cd replacement.",
            None,
        ),
        Tool::Gh => (
            "gh — GitHub CLI",
            "Installs the GitHub CLI and signs you in via the OAuth device flow — shows a URL + code to open in your browser, then completes login automatically.",
            Some("attach the panel (F2) to read the URL and the one-time code"),
        ),
        Tool::CargoWatch => (
            "cargo-watch — Cargo Auto-Rerun",
            "Watches your Rust project and re-runs `cargo check`/`run`/`test` on file changes. Installed via `cargo install`.",
            Some("requires rust/cargo installed first"),
        ),
        Tool::SqlxCli => (
            "sqlx-cli — SQLx Command Line",
            "CLI for managing SQLx database migrations and schema. Built with rustls + Postgres features, no native TLS dependency.",
            Some("requires rust/cargo installed first"),
        ),
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────
fn textwrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current.clone());
            current = word.to_string();
        }
    }
    if !current.is_empty() { lines.push(current); }
    lines
}
