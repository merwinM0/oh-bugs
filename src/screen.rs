//! 屏幕缓冲区（Screen Buffer）
//!
//! 使用 `rows × cols` 的二维 Cell 网格统一记录终端文本与虫子覆盖信息。
//! 每个 Cell 包含：
//!   - original_char：终端在该位置的原始字符
//!   - current_char：当前显示的字符（被虫子覆盖时不同）
//!   - bug_id：占用此格子的虫子 ID（None = 无虫子）
//!   - attributes：颜色、样式等视觉属性
//!
//! 虫子 Emoji（🦟🪰🐝🕷️🦗）在大多数终端中占 2 列宽度，
//! 因此虫子 API 同时操作 (x, y) 和 (x+1, y) 两个单元格。

/// 单元格视觉属性（颜色、样式等）
///
/// 目前 PTY 输出解析尚未填充这些字段，保留为后续 SGR 解析预留。
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct CellAttributes {
    /// 粗体
    pub bold: bool,
    /// 前景色（ANSI 色号，None = 默认）
    pub fg_color: Option<u8>,
    /// 背景色（ANSI 色号，None = 默认）
    pub bg_color: Option<u8>,
}

/// 屏幕上的一个单元格：记录终端字符、虫子占用与视觉属性
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Cell {
    /// 终端输出的原始字符（始终由 PTY 输出更新）
    pub original_char: char,
    /// 当前显示的字符（被虫子覆盖时设为虫子的首字符）
    pub current_char: char,
    /// 占用此格子的虫子 ID（None = 未被虫子占用）
    pub bug_id: Option<u64>,
    /// 视觉属性（颜色、样式等）
    pub attributes: CellAttributes,
}

impl Cell {
    fn new(ch: char) -> Self {
        Self {
            original_char: ch,
            current_char: ch,
            bug_id: None,
            attributes: CellAttributes::default(),
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

    /// 在光标位置放置一个字符：
    /// - 始终更新 `original_char`
    /// - 仅在无虫子占用时更新 `current_char`
    fn put_char(&mut self, ch: char) {
        if self.cursor_x < self.cols && self.cursor_y < self.rows {
            let row = self.cursor_y as usize;
            let col = self.cursor_x as usize;
            let cell = &mut self.cells[row][col];
            cell.original_char = ch;
            if cell.bug_id.is_none() {
                cell.current_char = ch;
            }
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
                                cell.original_char = ' ';
                                if cell.bug_id.is_none() {
                                    cell.current_char = ' ';
                                }
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

    /// 清除矩形区域（original_char 设为空格，无虫子时 current_char 同步更新）
    fn clear_region(&mut self, x1: u16, y1: u16, x2: u16, y2: u16) {
        for r in y1..=y2 {
            if r >= self.rows {
                break;
            }
            for c in x1..=x2 {
                if c >= self.cols {
                    break;
                }
                let cell = &mut self.cells[r as usize][c as usize];
                cell.original_char = ' ';
                if cell.bug_id.is_none() {
                    cell.current_char = ' ';
                }
            }
        }
    }

    // ─── Bug 2-Column Width API ───
    //
    // 虫子 Emoji（🦟🪰🐝🕷️🦗）在大多数终端中占 2 列宽度。
    // save_bug 在 (x, y) 和 (x+1, y) 记录虫子 ID；
    // restore_bug 清除标记、恢复 current_char，返回原始字符供终端重绘。

    /// 标记虫子占用的两个单元格 (x, y) 和 (x+1, y)。
    ///
    /// - 左单元格 `current_char` 设为虫子的首字符
    /// - 右单元格 `current_char` 设为 `'\0'`（占位标记）
    /// - 两个单元格的 `bug_id` 均设为 `Some(id)`
    pub fn save_bug(&mut self, x: u16, y: u16, bug_id: u64, emoji: &str) {
        let emoji_first = emoji.chars().next().unwrap_or(' ');
        if y < self.rows {
            if x < self.cols {
                let cell = &mut self.cells[y as usize][x as usize];
                cell.current_char = emoji_first;
                cell.bug_id = Some(bug_id);
            }
            if x + 1 < self.cols {
                let cell = &mut self.cells[y as usize][x as usize + 1];
                cell.current_char = '\0';
                cell.bug_id = Some(bug_id);
            }
        }
    }

    /// 恢复虫子占用的两个单元格 (x, y) 和 (x+1, y)。
    ///
    /// 清除 `bug_id`、恢复 `current_char = original_char`。
    /// 返回 `[left_original, right_original]` 供终端重绘。
    pub fn restore_bug(&mut self, x: u16, y: u16, bug_id: u64) -> [char; 2] {
        let mut chars = [' ', ' '];
        if y < self.rows {
            if x < self.cols {
                let cell = &mut self.cells[y as usize][x as usize];
                if cell.bug_id == Some(bug_id) {
                    chars[0] = cell.original_char;
                    cell.current_char = cell.original_char;
                    cell.bug_id = None;
                }
            }
            if x + 1 < self.cols {
                let cell = &mut self.cells[y as usize][x as usize + 1];
                if cell.bug_id == Some(bug_id) {
                    chars[1] = cell.original_char;
                    cell.current_char = cell.original_char;
                    cell.bug_id = None;
                }
            }
        }
        chars
    }

    /// 获取某位置的当前显示字符
    #[allow(dead_code)]
    pub fn get_char(&self, x: u16, y: u16) -> char {
        if x < self.cols && y < self.rows {
            self.cells[y as usize][x as usize].current_char
        } else {
            ' '
        }
    }

    /// 获取某位置的原始字符
    #[allow(dead_code)]
    pub fn get_original_char(&self, x: u16, y: u16) -> char {
        if x < self.cols && y < self.rows {
            self.cells[y as usize][x as usize].original_char
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
