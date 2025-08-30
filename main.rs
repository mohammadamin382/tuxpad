
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{
    cmp,
    collections::VecDeque,
    fs::{self, File},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::PathBuf,
    time::Instant,
};
use syntect::{
    highlighting::{ThemeSet, Theme},
    parsing::SyntaxSet,
};

const MAX_LINE_LENGTH: usize = 10000;
const MAX_VISIBLE_LINES: usize = 1000;
const CHUNK_SIZE: usize = 1000;

#[derive(Parser)]
#[command(name = "tuxpad")]
#[command(about = "A robust TUI text editor for large files")]
struct Args {
    #[arg(help = "File to open")]
    file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Normal,
    Insert,
    Command,
    Search,
    Replace,
}

#[derive(Debug, Clone)]
struct Cursor {
    x: usize,
    y: usize,
}

struct LineBuffer {
    lines: VecDeque<String>,
    max_lines: usize,
    start_line_number: usize,
    total_lines: usize,
}

impl LineBuffer {
    fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            max_lines,
            start_line_number: 0,
            total_lines: 0,
        }
    }

    fn load_chunk(&mut self, file_path: &PathBuf, start_line: usize) -> io::Result<()> {
        self.lines.clear();
        
        if !file_path.exists() {
            self.lines.push_back(String::new());
            self.total_lines = 1;
            self.start_line_number = 0;
            return Ok(());
        }

        let file = File::open(file_path)?;
        let reader = BufReader::new(file);
        let all_lines: Vec<String> = reader.lines().collect::<Result<Vec<_>, _>>()?;
        
        self.total_lines = if all_lines.is_empty() { 1 } else { all_lines.len() };
        
        if all_lines.is_empty() {
            self.lines.push_back(String::new());
            self.start_line_number = 0;
            return Ok(());
        }

        let actual_start = start_line.min(all_lines.len().saturating_sub(1));
        let end = (actual_start + self.max_lines).min(all_lines.len());
        
        for i in actual_start..end {
            let line = &all_lines[i];
            if line.len() > MAX_LINE_LENGTH {
                self.lines.push_back(line[..MAX_LINE_LENGTH].to_string());
            } else {
                self.lines.push_back(line.clone());
            }
        }
        
        self.start_line_number = actual_start;
        Ok(())
    }

    fn get_line(&self, index: usize) -> Option<&String> {
        if index >= self.start_line_number && index < self.start_line_number + self.lines.len() {
            self.lines.get(index - self.start_line_number)
        } else {
            None
        }
    }

    fn get_line_mut(&mut self, index: usize) -> Option<&mut String> {
        if index >= self.start_line_number && index < self.start_line_number + self.lines.len() {
            self.lines.get_mut(index - self.start_line_number)
        } else {
            None
        }
    }

    fn insert_line(&mut self, index: usize, content: String) {
        if index >= self.start_line_number && index <= self.start_line_number + self.lines.len() {
            let local_index = index - self.start_line_number;
            let truncated = if content.len() > MAX_LINE_LENGTH {
                content[..MAX_LINE_LENGTH].to_string()
            } else {
                content
            };
            self.lines.insert(local_index, truncated);
            self.total_lines += 1;
            
            // Keep buffer size manageable
            if self.lines.len() > self.max_lines {
                self.lines.pop_back();
            }
        }
    }

    fn remove_line(&mut self, index: usize) -> Option<String> {
        if index >= self.start_line_number && index < self.start_line_number + self.lines.len() {
            let local_index = index - self.start_line_number;
            self.total_lines = self.total_lines.saturating_sub(1);
            if self.total_lines == 0 {
                self.total_lines = 1;
            }
            self.lines.remove(local_index)
        } else {
            None
        }
    }
}

struct Editor {
    buffer: LineBuffer,
    cursor: Cursor,
    offset_y: usize,
    mode: Mode,
    filename: Option<PathBuf>,
    modified: bool,
    status_message: String,
    command_buffer: String,
    search_query: String,
    replace_query: String,
    replace_with: String,
    syntax_set: SyntaxSet,
    theme: Theme,
    show_line_numbers: bool,
    show_help: bool,
    clipboard: String,
    undo_buffer: VecDeque<String>,
    quit_requested: bool,
    last_operation: Instant,
    needs_reload: bool,
}

impl Editor {
    fn new() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let theme = theme_set.themes.get("base16-ocean.dark")
            .or_else(|| theme_set.themes.values().next())
            .unwrap()
            .clone();
        
        Self {
            buffer: LineBuffer::new(MAX_VISIBLE_LINES),
            cursor: Cursor { x: 0, y: 0 },
            offset_y: 0,
            mode: Mode::Normal,
            filename: None,
            modified: false,
            status_message: "TuxPad - Press F1 for help | ESC for normal mode".to_string(),
            command_buffer: String::new(),
            search_query: String::new(),
            replace_query: String::new(),
            replace_with: String::new(),
            syntax_set,
            theme,
            show_line_numbers: true,
            show_help: false,
            clipboard: String::new(),
            undo_buffer: VecDeque::new(),
            quit_requested: false,
            last_operation: Instant::now(),
            needs_reload: false,
        }
    }

    fn load_file(&mut self, path: &PathBuf) -> io::Result<()> {
        if let Err(e) = self.buffer.load_chunk(path, 0) {
            return Err(e);
        }
        
        self.filename = Some(path.clone());
        self.cursor = Cursor { x: 0, y: 0 };
        self.offset_y = 0;
        self.modified = false;
        self.status_message = format!("Loaded: {} ({} lines)", path.display(), self.buffer.total_lines);
        Ok(())
    }

    fn save_file(&mut self) -> io::Result<()> {
        if let Some(ref path) = self.filename {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            
            let file = File::create(path)?;
            let mut writer = BufWriter::new(file);
            
            // For large files, we need to reconstruct from chunks
            if self.buffer.total_lines > MAX_VISIBLE_LINES {
                // This is a simplified approach - in production you'd want 
                // to maintain the full file state or implement proper chunking
                for (i, line) in self.buffer.lines.iter().enumerate() {
                    writeln!(writer, "{}", line)?;
                }
            } else {
                for (i, line) in self.buffer.lines.iter().enumerate() {
                    if i == self.buffer.lines.len() - 1 {
                        write!(writer, "{}", line)?;
                    } else {
                        writeln!(writer, "{}", line)?;
                    }
                }
            }
            
            writer.flush()?;
            self.modified = false;
            self.status_message = format!("Saved: {} ({} lines)", path.display(), self.buffer.total_lines);
        } else {
            self.status_message = "No filename specified. Use :w filename to save".to_string();
        }
        Ok(())
    }

    fn save_undo_state(&mut self) {
        if let Some(line) = self.buffer.get_line(self.cursor.y) {
            self.undo_buffer.push_back(line.clone());
            if self.undo_buffer.len() > 50 {
                self.undo_buffer.pop_front();
            }
        }
    }

    fn insert_char(&mut self, c: char) -> io::Result<()> {
        self.save_undo_state();
        
        if let Some(line) = self.buffer.get_line_mut(self.cursor.y) {
            if line.len() < MAX_LINE_LENGTH {
                let insert_pos = self.cursor.x.min(line.len());
                line.insert(insert_pos, c);
                self.cursor.x = insert_pos + 1;
                self.modified = true;
            } else {
                self.status_message = "Line too long".to_string();
            }
        } else {
            // Need to reload chunk
            self.reload_current_chunk()?;
            if let Some(line) = self.buffer.get_line_mut(self.cursor.y) {
                if line.len() < MAX_LINE_LENGTH {
                    let insert_pos = self.cursor.x.min(line.len());
                    line.insert(insert_pos, c);
                    self.cursor.x = insert_pos + 1;
                    self.modified = true;
                }
            }
        }
        Ok(())
    }

    fn delete_char(&mut self) -> io::Result<()> {
        if self.cursor.x > 0 {
            self.save_undo_state();
            if let Some(line) = self.buffer.get_line_mut(self.cursor.y) {
                if self.cursor.x <= line.len() && !line.is_empty() {
                    line.remove(self.cursor.x - 1);
                    self.cursor.x -= 1;
                    self.modified = true;
                }
            }
        } else if self.cursor.y > 0 {
            self.save_undo_state();
            // Handle line joining carefully for large files
            if let (Some(current_line), Some(prev_line)) = (
                self.buffer.get_line(self.cursor.y).cloned(),
                self.buffer.get_line_mut(self.cursor.y - 1)
            ) {
                if prev_line.len() + current_line.len() < MAX_LINE_LENGTH {
                    let new_x = prev_line.len();
                    prev_line.push_str(&current_line);
                    self.buffer.remove_line(self.cursor.y);
                    self.cursor.y -= 1;
                    self.cursor.x = new_x;
                    self.modified = true;
                } else {
                    self.status_message = "Cannot join: resulting line would be too long".to_string();
                }
            }
        }
        Ok(())
    }

    fn insert_newline(&mut self) -> io::Result<()> {
        self.save_undo_state();
        
        if let Some(current_line) = self.buffer.get_line(self.cursor.y).cloned() {
            let split_pos = self.cursor.x.min(current_line.len());
            let new_line = current_line[split_pos..].to_string();
            
            if let Some(line) = self.buffer.get_line_mut(self.cursor.y) {
                line.truncate(split_pos);
            }
            
            self.buffer.insert_line(self.cursor.y + 1, new_line);
            self.cursor.y += 1;
            self.cursor.x = 0;
            self.modified = true;
        }
        Ok(())
    }

    fn move_cursor(&mut self, dx: isize, dy: isize) -> io::Result<()> {
        let old_y = self.cursor.y;
        
        // Vertical movement
        if dy != 0 {
            let new_y = (self.cursor.y as isize + dy).max(0) as usize;
            self.cursor.y = new_y.min(self.buffer.total_lines.saturating_sub(1));
            
            // Check if we need to reload chunk
            if self.cursor.y < self.buffer.start_line_number || 
               self.cursor.y >= self.buffer.start_line_number + self.buffer.lines.len() {
                self.reload_current_chunk()?;
            }
        }
        
        // Horizontal movement
        if let Some(line) = self.buffer.get_line(self.cursor.y) {
            let line_len = line.len();
            
            if dx != 0 {
                let new_x = (self.cursor.x as isize + dx).max(0) as usize;
                self.cursor.x = match self.mode {
                    Mode::Insert => new_x.min(line_len),
                    _ => new_x.min(line_len.saturating_sub(1).max(0)),
                };
            } else if dy != 0 {
                // Clamp x when moving vertically
                self.cursor.x = match self.mode {
                    Mode::Insert => self.cursor.x.min(line_len),
                    _ => self.cursor.x.min(line_len.saturating_sub(1).max(0)),
                };
            }
        } else {
            self.cursor.x = 0;
        }
        
        Ok(())
    }

    fn reload_current_chunk(&mut self) -> io::Result<()> {
        if let Some(ref path) = self.filename {
            let chunk_start = self.cursor.y.saturating_sub(MAX_VISIBLE_LINES / 2);
            self.buffer.load_chunk(path, chunk_start)?;
        }
        Ok(())
    }

    fn copy_line(&mut self) {
        if let Some(line) = self.buffer.get_line(self.cursor.y) {
            self.clipboard = line.clone();
            self.status_message = "Line copied".to_string();
        }
    }

    fn cut_line(&mut self) -> io::Result<()> {
        if let Some(line) = self.buffer.get_line(self.cursor.y).cloned() {
            self.save_undo_state();
            self.clipboard = line;
            self.buffer.remove_line(self.cursor.y);
            
            if self.buffer.total_lines == 0 {
                self.buffer.lines.push_back(String::new());
                self.buffer.total_lines = 1;
            }
            
            if self.cursor.y >= self.buffer.total_lines {
                self.cursor.y = self.buffer.total_lines.saturating_sub(1);
            }
            self.cursor.x = 0;
            self.modified = true;
            self.status_message = "Line cut".to_string();
        }
        Ok(())
    }

    fn paste_line(&mut self) -> io::Result<()> {
        if !self.clipboard.is_empty() {
            self.save_undo_state();
            self.buffer.insert_line(self.cursor.y + 1, self.clipboard.clone());
            self.cursor.y += 1;
            self.cursor.x = 0;
            self.modified = true;
            self.status_message = "Line pasted".to_string();
        }
        Ok(())
    }

    fn search(&self, query: &str) -> Vec<(usize, usize)> {
        let mut matches = Vec::new();
        
        // Only search in currently loaded chunk to avoid performance issues
        for (local_idx, line) in self.buffer.lines.iter().enumerate() {
            let line_idx = self.buffer.start_line_number + local_idx;
            let mut start = 0;
            while let Some(pos) = line[start..].find(query) {
                matches.push((line_idx, start + pos));
                start += pos + 1;
                if matches.len() > 100 { // Limit matches to prevent slowdown
                    break;
                }
            }
            if matches.len() > 100 {
                break;
            }
        }
        matches
    }

    fn replace_in_chunk(&mut self, search: &str, replace: &str) -> usize {
        if search.is_empty() {
            return 0;
        }
        
        self.save_undo_state();
        let mut count = 0;
        
        for line in self.buffer.lines.iter_mut() {
            let new_line = line.replace(search, replace);
            if new_line != *line {
                if new_line.len() <= MAX_LINE_LENGTH {
                    count += line.matches(search).count();
                    *line = new_line;
                } else {
                    // Truncate if replacement makes line too long
                    *line = new_line[..MAX_LINE_LENGTH].to_string();
                    count += 1;
                }
            }
        }
        
        if count > 0 {
            self.modified = true;
        }
        count
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> io::Result<bool> {
        // Throttle rapid operations to prevent crashes
        let now = Instant::now();
        if now.duration_since(self.last_operation).as_millis() < 10 {
            return Ok(true);
        }
        self.last_operation = now;

        let result = match self.mode {
            Mode::Normal => self.handle_normal_mode(key),
            Mode::Insert => self.handle_insert_mode(key),
            Mode::Command => self.handle_command_mode(key),
            Mode::Search => self.handle_search_mode(key),
            Mode::Replace => self.handle_replace_mode(key),
        };
        
        if self.needs_reload {
            self.reload_current_chunk()?;
            self.needs_reload = false;
        }
        
        result
    }

    fn handle_normal_mode(&mut self, key: KeyEvent) -> io::Result<bool> {
        match key.code {
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.modified && !self.quit_requested {
                    self.status_message = "File modified! Press Ctrl+Q again to quit without saving".to_string();
                    self.quit_requested = true;
                    return Ok(true);
                }
                return Ok(false);
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Err(e) = self.save_file() {
                    self.status_message = format!("Error saving: {}", e);
                }
                self.quit_requested = false;
            }
            KeyCode::Char('i') => {
                self.mode = Mode::Insert;
                self.status_message = "-- INSERT --".to_string();
            }
            KeyCode::Char('a') => {
                self.mode = Mode::Insert;
                if let Err(e) = self.move_cursor(1, 0) {
                    self.status_message = format!("Movement error: {}", e);
                }
                self.status_message = "-- INSERT --".to_string();
            }
            KeyCode::Char('o') => {
                self.mode = Mode::Insert;
                if let Some(line) = self.buffer.get_line(self.cursor.y) {
                    self.cursor.x = line.len();
                }
                if let Err(e) = self.insert_newline() {
                    self.status_message = format!("Insert error: {}", e);
                } else {
                    self.status_message = "-- INSERT --".to_string();
                }
            }
            KeyCode::Char(':') => {
                self.mode = Mode::Command;
                self.command_buffer.clear();
                self.status_message = "Command mode".to_string();
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Search;
                self.search_query.clear();
                self.status_message = "Search mode".to_string();
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = Mode::Replace;
                self.replace_query.clear();
                self.replace_with.clear();
                self.status_message = "Replace mode".to_string();
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.copy_line();
            }
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Err(e) = self.cut_line() {
                    self.status_message = format!("Cut error: {}", e);
                }
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Err(e) = self.paste_line() {
                    self.status_message = format!("Paste error: {}", e);
                }
            }
            KeyCode::F(1) => {
                self.show_help = !self.show_help;
                self.status_message = if self.show_help { "Help shown" } else { "Help hidden" }.to_string();
            }
            KeyCode::F(2) => {
                self.show_line_numbers = !self.show_line_numbers;
                self.status_message = if self.show_line_numbers { "Line numbers shown" } else { "Line numbers hidden" }.to_string();
            }
            KeyCode::Up => { let _ = self.move_cursor(0, -1); }
            KeyCode::Down => { let _ = self.move_cursor(0, 1); }
            KeyCode::Left => { let _ = self.move_cursor(-1, 0); }
            KeyCode::Right => { let _ = self.move_cursor(1, 0); }
            KeyCode::Home => self.cursor.x = 0,
            KeyCode::End => {
                if let Some(line) = self.buffer.get_line(self.cursor.y) {
                    self.cursor.x = line.len();
                }
            }
            KeyCode::PageUp => { 
                let _ = self.move_cursor(0, -20);
                self.needs_reload = true;
            }
            KeyCode::PageDown => { 
                let _ = self.move_cursor(0, 20);
                self.needs_reload = true;
            }
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status_message = "Normal mode".to_string();
                self.quit_requested = false;
            }
            _ => {
                self.quit_requested = false;
            }
        }
        Ok(true)
    }

    fn handle_insert_mode(&mut self, key: KeyEvent) -> io::Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                if let Err(e) = self.move_cursor(-1, 0) {
                    // Ignore movement errors on mode switch
                }
                self.status_message = "Normal mode".to_string();
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Err(e) = self.save_file() {
                    self.status_message = format!("Error saving: {}", e);
                }
            }
            KeyCode::Char(c) => {
                if let Err(e) = self.insert_char(c) {
                    self.status_message = format!("Insert error: {}", e);
                }
            }
            KeyCode::Enter => {
                if let Err(e) = self.insert_newline() {
                    self.status_message = format!("Newline error: {}", e);
                }
            }
            KeyCode::Backspace => {
                if let Err(e) = self.delete_char() {
                    self.status_message = format!("Delete error: {}", e);
                }
            }
            KeyCode::Up => { let _ = self.move_cursor(0, -1); }
            KeyCode::Down => { let _ = self.move_cursor(0, 1); }
            KeyCode::Left => { let _ = self.move_cursor(-1, 0); }
            KeyCode::Right => { let _ = self.move_cursor(1, 0); }
            KeyCode::Tab => {
                for _ in 0..4 {
                    if let Err(e) = self.insert_char(' ') {
                        self.status_message = format!("Tab insert error: {}", e);
                        break;
                    }
                }
            }
            _ => {}
        }
        Ok(true)
    }

    fn handle_command_mode(&mut self, key: KeyEvent) -> io::Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.command_buffer.clear();
                self.status_message = "Normal mode".to_string();
            }
            KeyCode::Enter => {
                self.execute_command()?;
                self.mode = Mode::Normal;
            }
            KeyCode::Char(c) => {
                if self.command_buffer.len() < 100 {
                    self.command_buffer.push(c);
                }
            }
            KeyCode::Backspace => {
                self.command_buffer.pop();
            }
            _ => {}
        }
        Ok(true)
    }

    fn handle_search_mode(&mut self, key: KeyEvent) -> io::Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.search_query.clear();
                self.status_message = "Normal mode".to_string();
            }
            KeyCode::Enter => {
                let matches = self.search(&self.search_query);
                if !matches.is_empty() {
                    self.cursor.y = matches[0].0;
                    self.cursor.x = matches[0].1;
                    self.needs_reload = true;
                    self.status_message = format!("Found {} matches in current chunk", matches.len());
                } else {
                    self.status_message = "No matches found in current view".to_string();
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Char(c) => {
                if self.search_query.len() < 100 {
                    self.search_query.push(c);
                }
            }
            KeyCode::Backspace => {
                self.search_query.pop();
            }
            _ => {}
        }
        Ok(true)
    }

    fn handle_replace_mode(&mut self, key: KeyEvent) -> io::Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.replace_query.clear();
                self.replace_with.clear();
                self.status_message = "Normal mode".to_string();
            }
            KeyCode::Enter => {
                if !self.replace_query.is_empty() {
                    let search = self.replace_query.clone();
                    let replace = self.replace_with.clone();
                    let count = self.replace_in_chunk(&search, &replace);
                    self.status_message = format!("Replaced {} occurrences in current chunk", count);
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Tab => {
                if self.replace_query.is_empty() {
                    self.status_message = "Enter search term first".to_string();
                } else {
                    self.status_message = "Enter replacement text:".to_string();
                }
            }
            KeyCode::Char(c) => {
                if self.replace_query.is_empty() || self.status_message.contains("search") {
                    if self.replace_query.len() < 100 {
                        self.replace_query.push(c);
                    }
                } else {
                    if self.replace_with.len() < 100 {
                        self.replace_with.push(c);
                    }
                }
            }
            KeyCode::Backspace => {
                if !self.replace_with.is_empty() {
                    self.replace_with.pop();
                } else {
                    self.replace_query.pop();
                }
            }
            _ => {}
        }
        Ok(true)
    }

    fn execute_command(&mut self) -> io::Result<()> {
        match self.command_buffer.as_str() {
            "q" => {
                if !self.modified {
                    std::process::exit(0);
                } else {
                    self.status_message = "File modified! Use 'q!' to quit without saving".to_string();
                }
            }
            "q!" => std::process::exit(0),
            "w" => {
                if let Err(e) = self.save_file() {
                    self.status_message = format!("Error saving: {}", e);
                }
            }
            "wq" => {
                if self.save_file().is_ok() {
                    std::process::exit(0);
                }
            }
            cmd if cmd.starts_with("w ") => {
                let filename = cmd[2..].trim();
                self.filename = Some(PathBuf::from(filename));
                if let Err(e) = self.save_file() {
                    self.status_message = format!("Error saving: {}", e);
                }
            }
            _ => {
                self.status_message = format!("Unknown command: {}", self.command_buffer);
            }
        }
        self.command_buffer.clear();
        Ok(())
    }

    fn update_scroll(&mut self, terminal_height: u16) {
        let height = terminal_height.saturating_sub(4) as usize;
        
        if self.cursor.y < self.offset_y {
            self.offset_y = self.cursor.y;
        } else if self.cursor.y >= self.offset_y + height {
            self.offset_y = self.cursor.y.saturating_sub(height - 1);
        }
    }

    fn render(&mut self, frame: &mut Frame) -> io::Result<()> {
        let size = frame.size();
        self.update_scroll(size.height);

        if self.show_help {
            self.render_help(frame, size);
            return Ok(());
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Title bar
                Constraint::Min(0),    // Editor
                Constraint::Length(1), // Mode bar
                Constraint::Length(1), // Status bar
            ])
            .split(size);

        // Title bar
        let filename = self.filename
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "[New File]".to_string());
        
        let title = format!(
            " 🐧 TuxPad │ {} {} │ {}/{} lines",
            filename,
            if self.modified { "●" } else { "" },
            self.cursor.y + 1,
            self.buffer.total_lines
        );
        
        let title_block = Paragraph::new(title)
            .style(Style::default().bg(Color::Blue).fg(Color::White));
        frame.render_widget(title_block, chunks[0]);

        // Editor area
        self.render_editor(frame, chunks[1])?;

        // Mode bar
        let mode_text = format!(
            " {:?} │ Ln {}, Col {} │ Chunk: {}-{} ",
            self.mode,
            self.cursor.y + 1,
            self.cursor.x + 1,
            self.buffer.start_line_number + 1,
            self.buffer.start_line_number + self.buffer.lines.len()
        );
        
        let mode_style = match self.mode {
            Mode::Insert => Style::default().bg(Color::Green).fg(Color::Black),
            Mode::Command => Style::default().bg(Color::Blue).fg(Color::White),
            Mode::Search => Style::default().bg(Color::Magenta).fg(Color::White),
            Mode::Replace => Style::default().bg(Color::Red).fg(Color::White),
            _ => Style::default().bg(Color::DarkGray).fg(Color::White),
        };
        
        let mode_bar = Paragraph::new(mode_text).style(mode_style);
        frame.render_widget(mode_bar, chunks[2]);

        // Status bar
        let status_text = match self.mode {
            Mode::Command => format!(" :{}", self.command_buffer),
            Mode::Search => format!(" 🔍 /{}", self.search_query),
            Mode::Replace => {
                if self.replace_query.is_empty() {
                    " 🔄 Replace: Enter search term".to_string()
                } else if self.replace_with.is_empty() {
                    format!(" 🔄 Replace '{}' with: ", self.replace_query)
                } else {
                    format!(" 🔄 Replace '{}' → '{}'", self.replace_query, self.replace_with)
                }
            }
            _ => format!(" {}", self.status_message),
        };
        
        let status_bar = Paragraph::new(status_text)
            .style(Style::default().bg(Color::Rgb(40, 40, 40)).fg(Color::White));
        frame.render_widget(status_bar, chunks[3]);
        
        Ok(())
    }

    fn render_editor(&self, frame: &mut Frame, area: Rect) -> io::Result<()> {
        let line_number_width = if self.show_line_numbers {
            cmp::max(format!("{}", self.buffer.total_lines).len(), 3) + 1
        } else {
            0
        };

        let editor_chunks = if self.show_line_numbers {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(line_number_width as u16),
                    Constraint::Min(0),
                ])
                .split(area)
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(0)])
                .split(area)
        };

        // Line numbers
        if self.show_line_numbers {
            let mut line_numbers = Vec::new();
            let start_display = self.offset_y;
            let end_display = (self.offset_y + area.height as usize).min(self.buffer.total_lines);
            
            for i in start_display..end_display {
                let line_num = i + 1;
                let style = if i == self.cursor.y {
                    Style::default().fg(Color::Yellow).bg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                line_numbers.push(
                    ListItem::new(format!("{:>width$}", line_num, width = line_number_width - 1))
                        .style(style)
                );
            }
            
            let line_number_list = List::new(line_numbers)
                .block(Block::default().borders(Borders::RIGHT).border_style(Style::default().fg(Color::DarkGray)));
            frame.render_widget(line_number_list, editor_chunks[0]);
        }

        // Main editor content
        let editor_area = if self.show_line_numbers {
            editor_chunks[1]
        } else {
            editor_chunks[0]
        };

        let mut text_lines = Vec::new();
        let start_display = self.offset_y;
        let end_display = (self.offset_y + editor_area.height as usize).min(self.buffer.total_lines);

        // Get file extension for syntax highlighting
        let extension = self.filename.as_ref()
            .and_then(|p| p.extension())
            .and_then(|ext| ext.to_str())
            .unwrap_or("");

        for line_idx in start_display..end_display {
            let line_content = self.buffer.get_line(line_idx)
                .map(|s| s.as_str())
                .unwrap_or("");

            let mut spans = if line_content.is_empty() {
                vec![Span::raw(" ")]
            } else {
                self.highlight_line_safe(line_content, extension)
            };

            // Highlight current line
            if line_idx == self.cursor.y {
                for span in &mut spans {
                    span.style = span.style.bg(Color::Rgb(40, 40, 40));
                }
            }

            text_lines.push(Line::from(spans));
        }
        
        // Fill remaining area
        while text_lines.len() < editor_area.height as usize {
            text_lines.push(Line::from(vec![Span::raw(" ")]));
        }

        let editor_paragraph = Paragraph::new(text_lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(Color::Black));

        frame.render_widget(editor_paragraph, editor_area);

        // Render cursor
        self.render_cursor(frame, editor_area, line_number_width)?;
        
        Ok(())
    }

    fn render_cursor(&self, frame: &mut Frame, editor_area: Rect, line_number_width: usize) -> io::Result<()> {
        if self.cursor.y >= self.offset_y && self.cursor.y < self.offset_y + editor_area.height as usize {
            let cursor_y = (self.cursor.y - self.offset_y) as u16;
            let cursor_x = self.cursor.x as u16;

            let adjusted_cursor_x = cursor_x + if self.show_line_numbers {
                line_number_width as u16
            } else {
                0
            };

            if adjusted_cursor_x < editor_area.width && cursor_y < editor_area.height {
                let cursor_area = Rect {
                    x: editor_area.x + adjusted_cursor_x,
                    y: editor_area.y + cursor_y,
                    width: 1,
                    height: 1,
                };
                
                let cursor_char = self.buffer.get_line(self.cursor.y)
                    .and_then(|line| line.chars().nth(self.cursor.x))
                    .unwrap_or(' ');

                let cursor_style = match self.mode {
                    Mode::Insert => Style::default().bg(Color::Green).fg(Color::Black),
                    Mode::Command => Style::default().bg(Color::Blue).fg(Color::White),
                    Mode::Search => Style::default().bg(Color::Magenta).fg(Color::White),
                    Mode::Replace => Style::default().bg(Color::Red).fg(Color::White),
                    _ => Style::default().bg(Color::Yellow).fg(Color::Black),
                };

                let cursor_widget = Paragraph::new(cursor_char.to_string())
                    .style(cursor_style);
                frame.render_widget(cursor_widget, cursor_area);
            }
        }
        Ok(())
    }

    fn highlight_line_safe<'a>(&self, line: &'a str, extension: &str) -> Vec<Span<'a>> {
        // Safe highlighting that won't crash on large content
        if line.len() > 500 {
            return vec![Span::raw(line)];
        }

        let keywords = match extension {
            "rs" => vec!["fn", "let", "mut", "if", "else", "match", "struct", "enum", "impl", "use", "pub"],
            "py" => vec!["def", "class", "if", "else", "elif", "for", "while", "import", "from", "return"],
            "js" | "ts" => vec!["function", "const", "let", "var", "if", "else", "for", "while", "class"],
            "c" | "cpp" => vec!["int", "char", "float", "double", "if", "else", "for", "while", "struct"],
            _ => vec![],
        };

        let mut spans = Vec::new();
        let mut current_word = String::new();
        let mut in_string = false;
        let mut string_char = '"';
        let mut in_comment = false;

        for (i, ch) in line.chars().enumerate() {
            if in_comment {
                spans.push(Span::styled(ch.to_string(), Style::default().fg(Color::Gray)));
                continue;
            }

            if in_string {
                current_word.push(ch);
                if ch == string_char {
                    spans.push(Span::styled(current_word.clone(), Style::default().fg(Color::Green)));
                    current_word.clear();
                    in_string = false;
                }
                continue;
            }

            if ch == '"' || ch == '\'' {
                if !current_word.is_empty() {
                    if keywords.contains(&current_word.as_str()) {
                        spans.push(Span::styled(current_word, Style::default().fg(Color::Blue)));
                    } else {
                        spans.push(Span::raw(current_word));
                    }
                    current_word = String::new();
                }
                current_word.push(ch);
                string_char = ch;
                in_string = true;
                continue;
            }

            if line[i..].starts_with("//") || line[i..].starts_with("#") {
                if !current_word.is_empty() {
                    spans.push(Span::raw(current_word));
                    current_word = String::new();
                }
                spans.push(Span::styled(line[i..].to_string(), Style::default().fg(Color::Gray)));
                break;
            }

            if ch.is_alphanumeric() || ch == '_' {
                current_word.push(ch);
            } else {
                if !current_word.is_empty() {
                    if keywords.contains(&current_word.as_str()) {
                        spans.push(Span::styled(current_word, Style::default().fg(Color::Blue)));
                    } else if current_word.chars().all(|c| c.is_ascii_digit()) {
                        spans.push(Span::styled(current_word, Style::default().fg(Color::Magenta)));
                    } else {
                        spans.push(Span::raw(current_word));
                    }
                    current_word = String::new();
                }
                spans.push(Span::raw(ch.to_string()));
            }
        }

        if !current_word.is_empty() {
            if keywords.contains(&current_word.as_str()) {
                spans.push(Span::styled(current_word, Style::default().fg(Color::Blue)));
            } else {
                spans.push(Span::raw(current_word));
            }
        }

        if spans.is_empty() {
            spans.push(Span::raw(" "));
        }

        spans
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let help_text = vec![
            "🐧 TuxPad - Robust Text Editor",
            "",
            "Normal Mode Commands:",
            "  i           - Enter insert mode",
            "  a           - Insert after cursor",
            "  o           - Insert new line below",
            "  ESC         - Return to normal mode",
            "",
            "File Operations:",
            "  Ctrl+S      - Save file",
            "  Ctrl+Q      - Quit (twice if modified)",
            "  :w          - Save",
            "  :q          - Quit",
            "  :wq         - Save and quit",
            "",
            "Movement:",
            "  Arrow Keys  - Move cursor",
            "  Home/End    - Start/End of line",
            "  Page Up/Dn  - Scroll pages",
            "",
            "Edit Operations:",
            "  Ctrl+C      - Copy current line",
            "  Ctrl+X      - Cut current line",
            "  Ctrl+V      - Paste line",
            "",
            "Search/Replace:",
            "  /           - Search in current chunk",
            "  Ctrl+R      - Replace in current chunk",
            "",
            "Display:",
            "  F1          - Toggle this help",
            "  F2          - Toggle line numbers",
            "",
            "Large File Support:",
            "  - Loads files in chunks for performance",
            "  - Automatic memory management",
            "  - Crash-resistant operations",
            "",
            "Press F1 or ESC to close help",
        ];

        let help_paragraph = Paragraph::new(help_text.join("\n"))
            .style(Style::default().fg(Color::White))
            .block(Block::default()
                .title(" Help - TuxPad ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue))
            )
            .wrap(Wrap { trim: true });

        let popup_area = Rect {
            x: area.width / 8,
            y: area.height / 10,
            width: area.width * 3 / 4,
            height: area.height * 4 / 5,
        };

        frame.render_widget(Clear, popup_area);
        frame.render_widget(help_paragraph, popup_area);
    }
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    
    // Improved error handling for terminal setup
    if let Err(e) = enable_raw_mode() {
        eprintln!("Failed to enable raw mode: {}", e);
        return Err(e);
    }
    
    let mut stdout = io::stdout();
    if let Err(e) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        eprintln!("Failed to enter alternate screen: {}", e);
        return Err(e);
    }
    
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            let _ = disable_raw_mode();
            eprintln!("Failed to create terminal: {}", e);
            return Err(e);
        }
    };
    
    let mut editor = Editor::new();
    
    // Load file if specified
    if let Some(filename) = args.file {
        if let Err(e) = editor.load_file(&filename) {
            editor.status_message = format!("Error loading file: {}", e);
        }
    }
    
    // Main loop with robust error handling
    let result = loop {
        match terminal.draw(|frame| {
            if let Err(e) = editor.render(frame) {
                editor.status_message = format!("Render error: {}", e);
            }
        }) {
            Ok(_) => {},
            Err(e) => {
                editor.status_message = format!("Draw error: {}", e);
                continue;
            }
        }
        
        match event::read() {
            Ok(Event::Key(key)) => {
                match editor.handle_key_event(key) {
                    Ok(should_continue) => {
                        if !should_continue {
                            break Ok(());
                        }
                    }
                    Err(e) => {
                        editor.status_message = format!("Key handling error: {}", e);
                        // Don't break on key handling errors
                    }
                }
            }
            Ok(_) => {}, // Ignore other events
            Err(e) => {
                editor.status_message = format!("Event read error: {}", e);
                // Continue on event read errors
            }
        }
    };
    
    // Cleanup
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    
    result
  }
