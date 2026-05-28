use crate::terminal::Terminal;
use std::time::{Duration, Instant};

/// 虫子 Emoji 池
const BUG_EMOJIS: &[&str] = &["🦟", "🪰", "🐝", "🕷️", "🦗"];

/// 单只虫子
#[derive(Debug, Clone)]
pub struct Bug {
    pub x: u16,
    pub y: u16,
    pub emoji: &'static str,
    pub birth: Instant,
    pub direction: i8, // -1: 向左, 1: 向右
    pub lifetime: Duration,
}

/// 虫子管理器（全局状态，线程安全）
pub struct BugManager {
    bugs: Vec<Bug>,
    max_concurrent: usize,
    cols: u16,
    rows: u16,
}

impl BugManager {
    pub fn new(max_concurrent: usize, cols: u16, rows: u16) -> Self {
        Self {
            bugs: Vec::new(),
            max_concurrent,
            cols,
            rows,
        }
    }

    /// 更新终端尺寸
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        // 移出超出边界的虫子
        self.bugs.retain(|b| b.x < cols && b.y < rows);
    }

    /// 触发一波虫子（根据匹配次数）
    pub fn trigger(&mut self, match_count: usize, lifetime: Duration) {
        if match_count == 0 {
            return;
        }
        // 计算本次要生成的虫子数量（每次匹配随机 2~5 只）
        let available = self.max_concurrent.saturating_sub(self.bugs.len());
        if available == 0 {
            return;
        }
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let mut rng = SimpleRng::new(seed);

        let mut to_spawn = 0;
        for _ in 0..match_count {
            let count = 2 + (rng.next() % 4) as usize; // 2~5
            to_spawn += count;
        }
        let to_spawn = to_spawn.min(available);

        for _ in 0..to_spawn {
            // 随机位置：行在视口上半区 (0 ~ rows/2)
            let y = (rng.next() % (self.rows / 2).max(1) as u64) as u16;
            let x = (rng.next() % self.cols.max(1) as u64) as u16;
            let emoji_idx = (rng.next() % BUG_EMOJIS.len() as u64) as usize;
            let direction = if rng.next() % 2 == 0 { 1 } else { -1 };

            self.bugs.push(Bug {
                x,
                y,
                emoji: BUG_EMOJIS[emoji_idx],
                birth: Instant::now(),
                direction,
                lifetime,
            });
        }
    }

    /// 更新虫子位置 & 移除超时虫子
    pub fn update(&mut self) {
        let now = Instant::now();
        // 先清除所有虫子的旧位置
        for bug in &self.bugs {
            let _ = Terminal::clear_at(bug.x, bug.y);
        }

        // 更新位置、移除超时虫子
        self.bugs.retain(|bug| {
            let elapsed = now.duration_since(bug.birth);
            elapsed < bug.lifetime
        });

        for bug in &mut self.bugs {
            // 水平漂移 1~2 列
            let drift = if bug.direction > 0 { 1 } else { 1 }; // 向右或向左
            let dx = drift;
            let new_x = bug.x as i32 + dx * bug.direction as i32;
            if new_x >= 0 && new_x < self.cols as i32 {
                bug.x = new_x as u16;
            } else {
                // 碰壁反弹
                bug.direction = -bug.direction;
                let new_x = bug.x as i32 + bug.direction as i32;
                if new_x >= 0 && new_x < self.cols as i32 {
                    bug.x = new_x as u16;
                }
            }

            // 随机上下浮动 0~1 行
            use std::time::{SystemTime, UNIX_EPOCH};
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            let mut rng = SimpleRng::new(seed.wrapping_add(bug.x as u64));
            let dy = (rng.next() % 2) as i16; // 0 或 1
            // 随机向上或向下
            let dy = if rng.next() % 2 == 0 { dy } else { -dy };
            let new_y = bug.y as i16 + dy;
            if new_y >= 0 && new_y < self.rows as i16 {
                bug.y = new_y as u16;
            }
        }
    }

    /// 绘制所有虫子
    pub fn draw(&self) {
        for bug in &self.bugs {
            let _ = Terminal::draw_bug(bug.x, bug.y, bug.emoji);
        }
        let _ = Terminal::flush();
    }

    /// 清除所有虫子
    pub fn clear_all(&mut self) {
        for bug in self.bugs.drain(..) {
            let _ = Terminal::clear_at(bug.x, bug.y);
        }
        let _ = Terminal::flush();
    }

    /// 获取当前虫子数量
    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.bugs.len()
    }
}

/// 简单的伪随机数生成器（线性同余）
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self { state: if seed == 0 { 1 } else { seed } }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.state >> 33
    }
}
