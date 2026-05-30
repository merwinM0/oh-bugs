//! 虫子动画引擎 — 屏幕覆盖模式
//!
//! 虫子是"漂浮在屏幕上"的视觉特效。覆盖文字时通过 ScreenBuffer 的
//! 二维 Cell 网格记录虫子 ID 与原始字符，移走时恢复。
//!
//! 每只虫子：
//!   - 由唯一 `id` 标识（通过 ScreenBuffer.Cell.bug_id 关联）
//!   - 记录 `old_x, old_y`（上一帧位置）用于恢复旧覆盖区
//!   - 水平和垂直各自独立的方向 `direction_x, direction_y`

use crate::screen::ScreenBuffer;
use crate::terminal::Terminal;

use std::io::Write;
use std::time::{Duration, Instant};

/// 虫子 Emoji 池（🦟🪰🐝🕷️🦗 在大多数终端中占 2 列宽度）
const BUG_EMOJIS: &[&str] = &["🦟", "🪰", "🐝", "🕷️", "🦗"];

/// 单只虫子
///
/// 虫子 Emoji 占 2 列宽度，覆盖 (x, y) 和 (x+1, y) 两个单元格。
/// 通过 `id` 与 ScreenBuffer.Cell.bug_id 关联，`old_x, old_y`
/// 记录上一帧位置供恢复之用。
#[derive(Debug, Clone)]
pub struct Bug {
    /// 唯一标识符（与 ScreenBuffer.Cell.bug_id 对应）
    pub id: u64,
    /// 当前位置（虫子左上角）
    pub x: u16,
    pub y: u16,
    /// 虫子 Emoji 字符串
    pub emoji: &'static str,
    /// 创建时间
    pub birth: Instant,
    /// 上一帧位置（用于恢复旧覆盖区）
    pub old_x: u16,
    pub old_y: u16,
    /// 水平移动方向：-1 向左，1 向右
    pub direction_x: i8,
    /// 垂直移动方向：-1 向上，1 向下
    pub direction_y: i8,
    /// 虫子 emoji 是否已写入 Cell 网格
    pub placed: bool,
    /// 生命周期（从 birth 起）
    pub lifetime: Duration,
    /// 随机相位，用于每只虫子独立的伪随机序列
    pub phase: u64,
}

/// 虫子管理器
pub struct BugManager {
    bugs: Vec<Bug>,
    max_concurrent: usize,
    /// 自增 ID 计数器，每创建一个虫子 +1
    next_id: u64,
    tick: u64,
}

impl BugManager {
    pub fn new(max_concurrent: usize, _cols: u16, _rows: u16) -> Self {
        Self {
            bugs: Vec::new(),
            max_concurrent,
            next_id: 1,
            tick: 0,
        }
    }

    /// 触发一波虫子（根据匹配次数）
    ///
    /// 每只虫子获得随机生命周期: [lifetime - 500ms, lifetime + 500ms]，至少 500ms
    /// 生成位置 x 被限制在 [0, cols-2]（确保 x+1 合法）。
    /// 刚创建的虫子尚未保存到 ScreenBuffer，由首次 `draw_to` 或下次 `update` 完成。
    pub fn trigger(&mut self, match_count: usize, lifetime: Duration, screen: &ScreenBuffer) {
        if match_count == 0 {
            return;
        }
        let available = self.max_concurrent.saturating_sub(self.bugs.len());
        if available == 0 {
            return;
        }
        let cols = screen.cols();
        let rows = screen.rows();
        if cols < 2 || rows == 0 {
            return;
        }

        // 用 tick + 实时时间混合作为种子，确保每次 trigger 产生不同结果
        let seed = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64)
            .wrapping_add(self.tick);
        let mut rng = SimpleRng::new(seed);
        self.tick = self.tick.wrapping_add(1);

        let mut to_spawn = 0;
        for _ in 0..match_count {
            let count = 2 + (rng.next() % 4) as usize; // 2~5
            to_spawn += count;
        }
        let to_spawn = to_spawn.min(available);

        let base_ms = lifetime.as_millis() as u64;
        let half = 500u64.min(base_ms);

        // 虫子占 2 列，最大 x = cols - 2
        let max_x = cols.saturating_sub(2).max(1);

        for _ in 0..to_spawn {
            let y = (rng.next() % (rows / 2).max(1) as u64) as u16;
            let x = (rng.next() % max_x as u64) as u16;
            let emoji_idx = (rng.next() % BUG_EMOJIS.len() as u64) as usize;
            let id = self.next_id;
            self.next_id += 1;

            let direction_x = if rng.next() % 2 == 0 { 1 } else { -1 };
            let direction_y = if rng.next() % 2 == 0 { 1 } else { -1 };

            // 每只虫子独立随机生命周期: 2~3 秒（基于配置中心值 ±500ms）
            let ms = base_ms - half + (rng.next() % (half * 2 + 1));
            let bug_lifetime = Duration::from_millis(ms.max(500));

            // 每只虫子独立随机相位，用于后续独立的移动轨迹
            let phase = rng.next();

            self.bugs.push(Bug {
                id,
                x,
                y,
                emoji: BUG_EMOJIS[emoji_idx],
                birth: Instant::now(),
                old_x: x,
                old_y: y,
                direction_x,
                direction_y,
                placed: false,
                lifetime: bug_lifetime,
                phase,
            });
        }
    }

    /// 每 tick 更新一次：恢复旧位置 → 移动 → 保存新位置 → 绘制
    pub fn update<W: Write>(
        &mut self,
        dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        self.tick = self.tick.wrapping_add(1);
        let now = Instant::now();
        let cols = screen.cols();
        let rows = screen.rows();
        if cols < 2 || rows == 0 {
            self.bugs.clear();
            return Ok(());
        }

        // 虫子占 2 列，最大 x = cols - 2，留 1 列上/下边距
        let max_x = cols.saturating_sub(2).max(1);
        let safe_rows = rows.saturating_sub(2);

        let mut alive = Vec::with_capacity(self.bugs.len());

        for mut bug in self.bugs.drain(..) {
            let elapsed = now.duration_since(bug.birth);

            if elapsed >= bug.lifetime {
                // ── 虫子超时：恢复 old 位置的原始字符 ──
                let [ch_left, ch_right] = screen.restore_bug(bug.old_x, bug.old_y, bug.id);
                if ch_left != ' ' || ch_right != ' ' {
                    Terminal::clear_at_to(dev, bug.old_x, bug.old_y, ch_left)?;
                    Terminal::clear_at_to(dev, bug.old_x + 1, bug.old_y, ch_right)?;
                }
                // 丢弃虫子
            } else {
                // ── 虫子还活着 ──

                // 1) 恢复 old 位置两个单元格的原始字符（飞走 → 文字还原）
                let [ch_left, ch_right] = screen.restore_bug(bug.old_x, bug.old_y, bug.id);
                if ch_left != ' ' || ch_right != ' ' {
                    Terminal::clear_at_to(dev, bug.old_x, bug.old_y, ch_left)?;
                    Terminal::clear_at_to(dev, bug.old_x + 1, bug.old_y, ch_right)?;
                }

                // 2) 随机移动
                let mut rng = SimpleRng::new(
                    self.tick
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(bug.phase),
                );

                // ── 水平移动 ──
                let dx = if bug.x < 3 && bug.direction_x == -1 {
                    bug.direction_x = 1;
                    1
                } else if bug.x >= max_x && bug.direction_x == 1 {
                    bug.direction_x = -1;
                    -1
                } else {
                    match rng.next() % 10 {
                        0..=5 => bug.direction_x as i32,
                        6..=8 => 0,
                        _ => -bug.direction_x as i32,
                    }
                };

                let new_x = bug.x as i32 + dx;
                if new_x <= 0 {
                    bug.x = 1;
                    bug.direction_x = 1;
                } else if new_x as u16 >= max_x {
                    bug.x = max_x;
                    bug.direction_x = -1;
                } else {
                    bug.x = new_x as u16;
                }

                // ── 垂直移动 ──
                let dy = if bug.y < 2 && bug.direction_y == -1 {
                    bug.direction_y = 1;
                    1i16
                } else if bug.y >= safe_rows && bug.direction_y == 1 {
                    bug.direction_y = -1;
                    -1i16
                } else {
                    match rng.next() % 10 {
                        0..=5 => bug.direction_y as i16,
                        6..=8 => 0i16,
                        _ => -bug.direction_y as i16,
                    }
                };

                let new_y = bug.y as i16 + dy;
                if new_y <= 0 {
                    bug.y = 1;
                    bug.direction_y = 1;
                } else if new_y as u16 >= rows {
                    bug.y = safe_rows.max(1);
                    bug.direction_y = -1;
                } else {
                    bug.y = new_y as u16;
                }

                // 3) 更新 old 位置（当前帧移动后，当前位置成为下一帧的 old）
                bug.old_x = bug.x;
                bug.old_y = bug.y;

                // 4) 保存新位置两个单元格到 ScreenBuffer（标记 bug_id）
                screen.save_bug(bug.x, bug.y, bug.id, bug.emoji);

                // 5) 在新位置绘制 Emoji（终端自动处理 2 列宽度）
                Terminal::draw_bug_to(dev, bug.x, bug.y, bug.emoji)?;

                alive.push(bug);
            }
        }

        self.bugs = alive;
        Ok(())
    }

    /// 为刚创建（`placed == false`）的虫子保存到 ScreenBuffer 并绘制 emoji。
    ///
    /// 通常在 `trigger` 之后立即调用。已有 placed 的虫子自动跳过。
    pub fn draw_to<W: Write>(
        &mut self,
        dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        for bug in &mut self.bugs {
            if !bug.placed {
                screen.save_bug(bug.x, bug.y, bug.id, bug.emoji);
                bug.placed = true;
                Terminal::draw_bug_to(dev, bug.x, bug.y, bug.emoji)?;
            }
        }
        Ok(())
    }

    /// 恢复所有虫子的原始字符（退出时调用）
    ///
    /// 使用每只虫子的 `old_x, old_y` 恢复（即最后保存的位置）。
    /// 仅对 `placed == true` 的虫子执行终端写入。
    pub fn clear_all<W: Write>(
        &mut self,
        dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        for bug in self.bugs.drain(..) {
            if bug.placed {
                let [ch_left, ch_right] =
                    screen.restore_bug(bug.old_x, bug.old_y, bug.id);
                Terminal::clear_at_to(dev, bug.old_x, bug.old_y, ch_left)?;
                Terminal::clear_at_to(dev, bug.old_x + 1, bug.old_y, ch_right)?;
            }
        }
        Ok(())
    }
}

/// 简单的伪随机数生成器
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state >> 33
    }
}
