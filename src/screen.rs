//! 屏幕缓冲区（Screen Buffer）
//!
//! 通过解析 PTY 输出流追踪终端每个位置的字符。
//! 虫子的覆盖/恢复数据由各 Bug 自身通过 `saved_left` / `saved_right` 自行管理，
//! ScreenBuffer 本身不感知虫子，只提供纯文本追踪。

/// 屏幕上的一个单元格
#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
}

impl Cell {
    fn new(ch: char) -> Self {
        Self { ch }
    }
}

/// 屏幕缓冲区，`rows × cols` 二维字符网格
pub struct ScreenBuffer {
    cells: Vec<Vec<Cell>>,
    cursor_x: u16,
    cursor_y: u16,
    cols: u16,
    rows: u16,
}

impl ScreenBuffer {
    pub fn new(cols: u16, rows: u16) -> Self {
        let cells = vec![vec![Cell::new(' '); cols as usize]; rows as usize];
        Self {
            cells,
            cursor_x: 0,
            cursor_y: 0,
            cols,
            rows,
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        let mut new_cells = vec![vec![Cell::new(' '); cols as usize]; rows as usize];
        for r in 0..self.rows.min(rows) as usize {
            for c in 0..self.cols.min(cols) as usize {
                new_cells[r][c] = self.cells[r][c].clone();
            }
        }
        self.cells = new_cells;
        self.cols = cols;
        self.rows = rows;
        self.cursor_x = self.cursor_x.min(cols.saturating_sub(1));
        self.cursor_y = self.cursor_y.min(rows.saturating_sub(1));
    }

    /// 处理从 PTY 输出的数据，更新屏幕缓冲区
    pub fn process_output(&mut self, data: &[u8]) {
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            if b == 0x1b {
                if let Some(len) = self.parse_escape(&data[i..]) {
                    i += len;
                } else {
                    i += 1;
                }
            } else {
                match b {
                    b'\n' => {
                        self.cursor_y = self.cursor_y.saturating_add(1);
                        if self.cursor_y >= self.rows {
                            self.scroll_up();
                            self.cursor_y = self.rows - 1;
                        }
                        i += 1;
                    }
                    b'\r' => {
                        self.cursor_x = 0;
                        i += 1;
                    }
                    b'\t' => {
                        let tab = 8 - (self.cursor_x % 8);
                        self.cursor_x =
                            (self.cursor_x + tab).min(self.cols.saturating_sub(1));
                        i += 1;
                    }
                    0x08 => {
                        if self.cursor_x > 0 {
                            self.cursor_x -= 1;
                        }
                        i += 1;
                    }
                    0x07 | 0x00 => {
                        i += 1;
                    }
                    _ if b >= 0x20 && b < 0x7f => {
                        self.put_char(b as char);
                        i += 1;
                    }
                    _ if b >= 0x80 => {
                        let remaining = &data[i..];
                        if let Ok(s) = std::str::from_utf8(remaining) {
                            if let Some(ch) = s.chars().next() {
                                let char_len = ch.len_utf8();
                                self.put_char(ch);
                                i += char_len;
                                continue;
                            }
                        }
                        i += 1;
                    }
                    _ => {
                        i += 1;
                    }
                }
            }
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.cursor_x < self.cols && self.cursor_y < self.rows {
            let row = self.cursor_y as usize;
            let col = self.cursor_x as usize;
            self.cells[row][col].ch = ch;
        }
        self.cursor_x += 1;
        if self.cursor_x >= self.cols {
            self.cursor_x = 0;
            self.cursor_y += 1;
            if self.cursor_y >= self.rows {
                self.scroll_up();
                self.cursor_y = self.rows - 1;
            }
        }
    }

    fn scroll_up(&mut self) {
        for r in 1..self.rows as usize {
            self.cells[r - 1] =
                std::mem::replace(&mut self.cells[r], vec![Cell::new(' '); self.cols as usize]);
        }
        self.cells[self.rows as usize - 1] = vec![Cell::new(' '); self.cols as usize];
    }

    fn parse_escape(&mut self, data: &[u8]) -> Option<usize> {
        if data.len() < 2 {
            return None;
        }
        match data[1] {
            b'[' => {
                let mut i = 2;
                let mut params = Vec::new();
                let mut current = 0i32;
                let mut _has_param = false;
                while i < data.len() {
                    let b = data[i];
                    match b {
                        b'0'..=b'9' => {
                            current = current * 10 + (b - b'0') as i32;
                            _has_param = true;
                        }
                        b';' => {
                            params.push(current);
                            current = 0;
                            _has_param = false;
                        }
                        b'?' => {
                            params.push(-1);
                        }
                        _ if (b'A'..=b'Z').contains(&b) || (b'a'..=b'z').contains(&b) => {
                            params.push(current);
                            self.handle_csi(b, &params);
                            return Some(i + 1);
                        }
                        _ => return Some(i + 1),
                    }
                    i += 1;
                }
                None
            }
            _ if data.len() >= 2 => Some(2),
            _ => None,
        }
    }

    fn handle_csi(&mut self, final_byte: u8, params: &[i32]) {
        let n = params.first().copied().unwrap_or(1);
        match final_byte {
            b'A' => self.cursor_y = self.cursor_y.saturating_sub(n.max(1) as u16),
            b'B' => {
                self.cursor_y =
                    (self.cursor_y + n.max(1) as u16).min(self.rows.saturating_sub(1))
            }
            b'C' => {
                self.cursor_x =
                    (self.cursor_x + n.max(1) as u16).min(self.cols.saturating_sub(1))
            }
            b'D' => self.cursor_x = self.cursor_x.saturating_sub(n.max(1) as u16),
            b'H' | b'f' => {
                if params.len() >= 2 {
                    let r = (params[0].max(1) - 1) as u16;
                    let c = (params[1].max(1) - 1) as u16;
                    self.cursor_y = r.min(self.rows.saturating_sub(1));
                    self.cursor_x = c.min(self.cols.saturating_sub(1));
                } else if params.len() == 1 {
                    let r = (params[0].max(1) - 1) as u16;
                    self.cursor_y = r.min(self.rows.saturating_sub(1));
                    self.cursor_x = 0;
                } else {
                    self.cursor_y = 0;
                    self.cursor_x = 0;
                }
            }
            b'J' => match n {
                0 | -1 => self.clear_region(self.cursor_x, self.cursor_y, self.cols - 1, self.rows - 1),
                1 => self.clear_region(0, 0, self.cursor_x, self.cursor_y),
                2 => {
                    for row in &mut self.cells {
                        for cell in row.iter_mut() {
                            cell.ch = ' ';
                        }
                    }
                }
                _ => {}
            },
            b'K' => match n {
                0 | -1 => self.clear_region(self.cursor_x, self.cursor_y, self.cols - 1, self.cursor_y),
                1 => self.clear_region(0, self.cursor_y, self.cursor_x, self.cursor_y),
                2 => self.clear_region(0, self.cursor_y, self.cols - 1, self.cursor_y),
                _ => {}
            },
            b'm' => {}
            b'L' | b'M' | b'P' | b'@' | b'X' | b'S' | b'T' => {}
            _ => {}
        }
    }

    fn clear_region(&mut self, x1: u16, y1: u16, x2: u16, y2: u16) {
        for r in y1..=y2 {
            if r >= self.rows {
                break;
            }
            for c in x1..=x2 {
                if c >= self.cols {
                    break;
                }
                self.cells[r as usize][c as usize].ch = ' ';
            }
        }
    }

    /// 获取某位置的当前字符（供虫子读取以保存）
    pub fn get_char(&self, x: u16, y: u16) -> char {
        if x < self.cols && y < self.rows {
            self.cells[y as usize][x as usize].ch
        } else {
            ' '
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }
}
