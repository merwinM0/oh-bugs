//! 屏幕缓冲区（Screen Buffer）
//!
//! 实时追踪终端屏幕上的字符内容，支持虫子动画的"保存→覆盖→恢复"流程。
//!
//! 原理：当 PTY 输出被转发到真实终端时，同步解析输出流中的
//! 可打印字符和控制序列，维护一个 `cols × rows` 的字符网格。
//! 虫子覆盖前从网格读取原字符保存，飞走后写回终端恢复。

use std::collections::HashMap;

/// 简化的屏幕缓冲区
pub struct ScreenBuffer {
    /// 网格字符 [row][col]，row 0 是最顶部
    chars: Vec<Vec<char>>,
    /// 当前光标位置
    cursor_x: u16,
    cursor_y: u16,
    /// 终端尺寸
    cols: u16,
    rows: u16,
    /// 被虫子保存的字符: (x, y) → original_char
    saved: HashMap<(u16, u16), char>,
}

impl ScreenBuffer {
    pub fn new(cols: u16, rows: u16) -> Self {
        let chars = vec![vec![' '; cols as usize]; rows as usize];
        Self {
            chars,
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
        let mut new_chars = vec![vec![' '; cols as usize]; rows as usize];
        for r in 0..self.rows.min(rows) as usize {
            for c in 0..self.cols.min(cols) as usize {
                new_chars[r][c] = self.chars[r][c];
            }
        }
        self.chars = new_chars;
        self.cols = cols;
        self.rows = rows;
        self.cursor_x = self.cursor_x.min(cols - 1);
        self.cursor_y = self.cursor_y.min(rows - 1);

        // 清理超出边界的 saved 项
        self.saved.retain(|&(x, y), _| x < cols && y < rows);
    }

    /// 处理从 PTY 输出的数据，更新屏幕缓冲区
    pub fn process_output(&mut self, data: &[u8]) {
        // 使用字节迭代器解析
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            if b == 0x1b {
                // ANSI 转义序列
                if let Some(len) = self.parse_escape(&data[i..]) {
                    i += len;
                } else {
                    i += 1;
                }
            } else {
                match b {
                    b'\n' => {
                        self.cursor_y = self.cursor_y.saturating_add(1);
                        // scroll if at bottom
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
                        self.cursor_x = (self.cursor_x + tab).min(self.cols - 1);
                        i += 1;
                    }
                    0x08 => {
                        // backspace
                        if self.cursor_x > 0 {
                            self.cursor_x -= 1;
                        }
                        i += 1;
                    }
                    0x07 | 0x00 => {
                        // bell, null — skip
                        i += 1;
                    }
                    _ if b >= 0x20 && b < 0x7f => {
                        // ASCII printable
                        self.put_char(b as char);
                        i += 1;
                    }
                    _ if b >= 0x80 => {
                        // UTF-8 multi-byte — decode one char
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

    /// 在指定位置放置一个字符
    fn put_char(&mut self, ch: char) {
        if self.cursor_x < self.cols && self.cursor_y < self.rows {
            let row = self.cursor_y as usize;
            let col = self.cursor_x as usize;
            self.chars[row][col] = ch;
        }
        self.cursor_x += 1;
        if self.cursor_x >= self.cols {
            // 自动换行
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
        // 移动 saved 键中的 y 坐标
        let mut new_saved = HashMap::new();
        for (&(x, y), &ch) in &self.saved {
            if y == 0 {
                // 被滚出屏幕了
                continue;
            }
            new_saved.insert((x, y - 1), ch);
        }
        self.saved = new_saved;

        // 上移字符网格
        for r in 1..self.rows as usize {
            self.chars[r - 1] = std::mem::replace(&mut self.chars[r], vec![' '; self.cols as usize]);
        }
        // 最后一行清空
        self.chars[self.rows as usize - 1] = vec![' '; self.cols as usize];
    }

    /// 解析 ANSI 转义序列，返回序列总长度（含 \x1b）
    fn parse_escape(&mut self, data: &[u8]) -> Option<usize> {
        if data.len() < 2 {
            return None;
        }
        match data[1] {
            b'[' => {
                // CSI 序列: \x1b[ ... <final byte>
                let mut i = 2;
                // 收集参数
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
                            // DEC private mode — skip type
                            params.push(-1);
                        }
                        _ if (b'A'..=b'Z').contains(&b) || (b'a'..=b'z').contains(&b) => {
                            params.push(current);
                            // 处理已知的 CSI 序列
                            self.handle_csi(b, &params);
                            return Some(i + 1);
                        }
                        _ => {
                            // 不支持的控制字符，跳过整个序列
                            return Some(i + 1);
                        }
                    }
                    i += 1;
                }
                // 不完整的序列
                None
            }
            // 其他转义序列（如 \x1bM = RI, \x1b7/8 = save/restore cursor）
            _ if data.len() >= 2 => Some(2),
            _ => None,
        }
    }

    /// 处理 CSI 序列
    fn handle_csi(&mut self, final_byte: u8, params: &[i32]) {
        let n = params.first().copied().unwrap_or(1);
        match final_byte {
            b'A' => {
                // CUU: Cursor Up
                self.cursor_y = self.cursor_y.saturating_sub(n.max(1) as u16);
            }
            b'B' => {
                // CUD: Cursor Down
                self.cursor_y = (self.cursor_y + n.max(1) as u16).min(self.rows - 1);
            }
            b'C' => {
                // CUF: Cursor Forward
                self.cursor_x = (self.cursor_x + n.max(1) as u16).min(self.cols - 1);
            }
            b'D' => {
                // CUB: Cursor Back
                self.cursor_x = self.cursor_x.saturating_sub(n.max(1) as u16);
            }
            b'H' | b'f' => {
                // CUP: Cursor Position
                if params.len() >= 2 {
                    let r = (params[0].max(1) - 1) as u16;
                    let c = (params[1].max(1) - 1) as u16;
                    self.cursor_y = r.min(self.rows - 1);
                    self.cursor_x = c.min(self.cols - 1);
                } else if params.len() == 1 {
                    let r = (params[0].max(1) - 1) as u16;
                    self.cursor_y = r.min(self.rows - 1);
                    self.cursor_x = 0;
                } else {
                    self.cursor_y = 0;
                    self.cursor_x = 0;
                }
            }
            b'J' => {
                // ED: Erase in Display
                match n {
                    0 | -1 => {
                        // Erase from cursor to end of screen
                        self.clear_region(
                            self.cursor_x,
                            self.cursor_y,
                            self.cols - 1,
                            self.rows - 1,
                        );
                    }
                    1 => {
                        // Erase from start to cursor
                        self.clear_region(0, 0, self.cursor_x, self.cursor_y);
                    }
                    2 => {
                        // Erase entire screen
                        for row in &mut self.chars {
                            for ch in row.iter_mut() {
                                *ch = ' ';
                            }
                        }
                    }
                    _ => {}
                }
            }
            b'K' => {
                // EL: Erase in Line
                match n {
                    0 | -1 => {
                        // Erase from cursor to end of line
                        self.clear_region(
                            self.cursor_x,
                            self.cursor_y,
                            self.cols - 1,
                            self.cursor_y,
                        );
                    }
                    1 => {
                        // Erase from start of line to cursor
                        self.clear_region(0, self.cursor_y, self.cursor_x, self.cursor_y);
                    }
                    2 => {
                        // Erase entire line
                        self.clear_region(0, self.cursor_y, self.cols - 1, self.cursor_y);
                    }
                    _ => {}
                }
            }
            b'm' => {
                // SGR: Select Graphic Rendition — 忽略颜色/样式
                // 不影响字符网格
            }
            b'L' | b'M' | b'P' | b'@' | b'X' | b'S' | b'T' => {
                // 插入/删除行/字符 — 这些比较复杂，忽略以保持简单
                // 对于常见的 shell 交互来说影响不大
            }
            _ => {
                // 其他 CSI 序列忽略
            }
        }
    }

    /// 清除矩形区域（将字符设为空格）
    fn clear_region(&mut self, x1: u16, y1: u16, x2: u16, y2: u16) {
        for r in y1..=y2 {
            if r >= self.rows {
                break;
            }
            for c in x1..=x2 {
                if c >= self.cols {
                    break;
                }
                self.chars[r as usize][c as usize] = ' ';
            }
        }
    }

    /// 保存指定位置的原始字符（被虫子覆盖前调用）
    pub fn save_char(&mut self, x: u16, y: u16) -> char {
        if x < self.cols && y < self.rows {
            let ch = self.chars[y as usize][x as usize];
            self.saved.entry((x, y)).or_insert(ch);
            ch
        } else {
            ' '
        }
    }

    /// 恢复指定位置的原始字符（虫子移走后调用）
    pub fn restore_char(&mut self, x: u16, y: u16) -> Option<char> {
        self.saved.remove(&(x, y))
    }

    /// 获取某位置的当前字符
    pub fn get_char(&self, x: u16, y: u16) -> char {
        if x < self.cols && y < self.rows {
            self.chars[y as usize][x as usize]
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
