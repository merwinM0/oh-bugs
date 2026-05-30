//! 屏幕缓冲区（Screen Buffer）
//!
//! 实时追踪终端屏幕上的字符内容，支持虫子动画的"保存→覆盖→恢复"流程。
//!
//! 使用 `rows × cols` 的二维 Cell 网格统一记录终端文本与虫子覆盖信息。
//! 虫子 Emoji（🦟🪰🐝🕷️🦗）在大多数终端中占 2 列宽度，
//! 因此保存/恢复操作同时处理 (x, y) 和 (x+1, y) 两个单元格。

use std::collections::HashMap;

/// 屏幕上的一个单元格：记录终端字符与虫子覆盖状态
#[derive(Debug, Clone)]
pub struct Cell {
    /// 终端输出的原始字符
    pub ch: char,
    /// 当前是否有虫子覆盖此单元格
    pub bug_covered: bool,
}

impl Cell {
    pub fn new(ch: char) -> Self {
        Self {
            ch,
            bug_covered: false,
        }
    }
}

/// 屏幕缓冲区，`rows × cols` 二维网格，统一记录终端文本与虫子覆盖信息
pub struct ScreenBuffer {
    /// 网格单元格 [row][col]
    cells: Vec<Vec<Cell>>,
    /// 当前光标位置
    cursor_x: u16,
    cursor_y: u16,
    /// 终端尺寸
    cols: u16,
    rows: u16,
    /// 被虫子覆盖前保存的原始字符: (x, y) → original_char
    saved: HashMap<(u16, u16), char>,
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
            saved: HashMap::new(),
        }
    }

    /// 调整终端尺寸
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

        // 清理超出边界的 saved 项
        self.saved.retain(|&(x, y), _| x < cols && y < rows);
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

    /// 在光标位置放置一个字符（只更新 `ch`，不改变 `bug_covered`）
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

    /// 屏幕上滚一行（所有内容上移，底部插入空行）
    fn scroll_up(&mut self) {
        // 上移 saved 键中的 y 坐标
        let mut new_saved = HashMap::new();
        for (&(x, y), &ch) in &self.saved {
            if y == 0 {
                continue;
            }
            new_saved.insert((x, y - 1), ch);
        }
        self.saved = new_saved;

        // 上移单元格网格
        for r in 1..self.rows as usize {
            self.cells[r - 1] = std::mem::replace(
                &mut self.cells[r],
                vec![Cell::new(' '); self.cols as usize],
            );
        }
        // 最后一行清空
        self.cells[self.rows as usize - 1] = vec![Cell::new(' '); self.cols as usize];
    }

    /// 解析 ANSI 转义序列，返回序列总长度（含 \x1b）
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
                        _ => {
                            return Some(i + 1);
                        }
                    }
                    i += 1;
                }
                None
            }
            _ if data.len() >= 2 => Some(2),
            _ => None,
        }
    }

    /// 处理 CSI 序列
    fn handle_csi(&mut self, final_byte: u8, params: &[i32]) {
        let n = params.first().copied().unwrap_or(1);
        match final_byte {
            b'A' => {
                self.cursor_y = self.cursor_y.saturating_sub(n.max(1) as u16);
            }
            b'B' => {
                self.cursor_y =
                    (self.cursor_y + n.max(1) as u16).min(self.rows.saturating_sub(1));
            }
            b'C' => {
                self.cursor_x =
                    (self.cursor_x + n.max(1) as u16).min(self.cols.saturating_sub(1));
            }
            b'D' => {
                self.cursor_x = self.cursor_x.saturating_sub(n.max(1) as u16);
            }
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
            b'J' => {
                match n {
                    0 | -1 => {
                        self.clear_region(
                            self.cursor_x,
                            self.cursor_y,
                            self.cols - 1,
                            self.rows - 1,
                        );
                    }
                    1 => {
                        self.clear_region(0, 0, self.cursor_x, self.cursor_y);
                    }
                    2 => {
                        for row in &mut self.cells {
                            for cell in row.iter_mut() {
                                cell.ch = ' ';
                            }
                        }
                    }
                    _ => {}
                }
            }
            b'K' => {
                match n {
                    0 | -1 => {
                        self.clear_region(
                            self.cursor_x,
                            self.cursor_y,
                            self.cols - 1,
                            self.cursor_y,
                        );
                    }
                    1 => {
                        self.clear_region(0, self.cursor_y, self.cursor_x, self.cursor_y);
                    }
                    2 => {
                        self.clear_region(0, self.cursor_y, self.cols - 1, self.cursor_y);
                    }
                    _ => {}
                }
            }
            b'm' => {}
            b'L' | b'M' | b'P' | b'@' | b'X' | b'S' | b'T' => {}
            _ => {}
        }
    }

    /// 清除矩形区域（将 `ch` 设为空格，不改变 `bug_covered`）
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

    // ─── Bug 2-Column Width API ───
    //
    // 虫子 Emoji（🦟🪰🐝🕷️🦗）在大多数终端中占 2 列宽度，
    // 因此每只虫子覆盖 (x, y) 和 (x+1, y) 两个单元格。

    /// 保存虫子覆盖位置 (x, y) 和 (x+1, y) 的原始字符，
    /// 并将两个单元格标记为 `bug_covered`。
    ///
    /// 返回 `[left_original, right_original]`。
    pub fn save_bug(&mut self, x: u16, y: u16) -> [char; 2] {
        let left = self.save_cell(x, y);
        let right = self.save_cell(x + 1, y);
        if y < self.rows {
            if x < self.cols {
                self.cells[y as usize][x as usize].bug_covered = true;
            }
            if x + 1 < self.cols {
                self.cells[y as usize][x as usize + 1].bug_covered = true;
            }
        }
        [left, right]
    }

    /// 恢复虫子覆盖位置 (x, y) 和 (x+1, y) 的原始字符，
    /// 并取消两个单元格的 `bug_covered` 标记。
    ///
    /// 返回 `[Some(left), Some(right)]`，若未被保存则为 `None`。
    pub fn restore_bug(&mut self, x: u16, y: u16) -> [Option<char>; 2] {
        let left = self.restore_cell(x, y);
        let right = self.restore_cell(x + 1, y);
        if y < self.rows {
            if x < self.cols {
                self.cells[y as usize][x as usize].bug_covered = false;
            }
            if x + 1 < self.cols {
                self.cells[y as usize][x as usize + 1].bug_covered = false;
            }
        }
        [left, right]
    }

    /// 检查某位置是否被虫子覆盖
    pub fn is_bug_covered(&self, x: u16, y: u16) -> bool {
        if x < self.cols && y < self.rows {
            self.cells[y as usize][x as usize].bug_covered
        } else {
            false
        }
    }

    /// 保存单个单元格的原始字符（内部使用，返回该位置的字符）
    fn save_cell(&mut self, x: u16, y: u16) -> char {
        if x < self.cols && y < self.rows {
            let ch = self.cells[y as usize][x as usize].ch;
            self.saved.entry((x, y)).or_insert(ch);
            ch
        } else {
            ' '
        }
    }

    /// 恢复单个单元格的原始字符（内部使用）
    fn restore_cell(&mut self, x: u16, y: u16) -> Option<char> {
        self.saved.remove(&(x, y))
    }

    /// 获取某位置的当前字符
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
