//! 虫子动画引擎 — 屏幕覆盖模式
//!
//! 虫子是"漂浮在屏幕上"的视觉特效。覆盖文字时：
//!   1. 从 ScreenBuffer 保存原始字符
//!   2. 在终端上绘制虫子 Emoji
//! 虫子移走/消失时：
//!   1. 从 ScreenBuffer 恢复原始字符
//!   2. 写回终端
//!
//! 每只虫子自己管理"保存→覆盖→恢复"的完整生命周期：
//!   - 虫子自己保存当前位置的原始字符
//!   - 虫子飞走时自己恢复旧位置的字符
//!   - 虫子飞到新位置时自己保存新位置的字符

use crate::screen::ScreenBuffer;
use crate::terminal::Terminal;

use std::io::Write;
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
    /// 虫子在当前位置覆盖的原始字符（用于恢复）
    /// 有值 = 虫子正覆盖在该位置，移动/消失前需恢复
    /// None   = 虫子还未保存位置（刚创建），或已恢复
    pub saved_char: Option<char>,
    /// 随机相位，用于每只虫子独立的伪随机序列
    pub phase: u64,
}

/// 虫子管理器
pub struct BugManager {
    bugs: Vec<Bug>,
    max_concurrent: usize,
    tick: u64,
}

impl BugManager {
    pub fn new(max_concurrent: usize, _cols: u16, _rows: u16) -> Self {
        Self {
            bugs: Vec::new(),
            max_concurrent,
            tick: 0,
        }
    }

    /// 触发一波虫子（根据匹配次数）
    ///
    /// 每只虫子获得随机生命周期: [lifetime - 500ms, lifetime + 500ms]，至少 500ms
    /// 刚创建的虫子 `saved_char = None`，由首次 `draw_to` 或下次 `update` 完成保存+绘制
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
        if cols == 0 || rows == 0 {
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

        for _ in 0..to_spawn {
            let y = (rng.next() % (rows / 2).max(1) as u64) as u16;
            let x = (rng.next() % cols.max(1) as u64) as u16;
            let emoji_idx = (rng.next() % BUG_EMOJIS.len() as u64) as usize;
            let direction = if rng.next() % 2 == 0 { 1 } else { -1 };

            // 每只虫子独立随机生命周期: 2~3 秒（基于配置中心值 ±500ms）
            let ms = base_ms - half + (rng.next() % (half * 2 + 1));
            let bug_lifetime = Duration::from_millis(ms.max(500));

            // 每只虫子独立随机相位，用于后续独立的移动轨迹
            let phase = rng.next();

            self.bugs.push(Bug {
                x,
                y,
                emoji: BUG_EMOJIS[emoji_idx],
                birth: Instant::now(),
                direction,
                lifetime: bug_lifetime,
                saved_char: None,
                phase,
            });
        }
    }

    /// 每 tick 更新一次：恢复旧位置 → 移动 → 保存新位置 → 绘制
    ///
    /// 每只虫子的完整生命周期：
    ///   1. 检查是否超时：恢复字符，丢弃
    ///   2. 存活：恢复旧位置字符 → 随机移动 → 保存新位置字符 → 绘制 Emoji
    ///
    /// 所有 Terminal 写入都在此方法内完成，不再需要外部调用 restore_positions。
    pub fn update<W: Write>(
        &mut self,
        dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        self.tick = self.tick.wrapping_add(1);
        let now = Instant::now();
        let cols = screen.cols();
        let rows = screen.rows();
        if cols == 0 || rows == 0 {
            self.bugs.clear();
            return Ok(());
        }

        let safe_cols = cols.saturating_sub(2);
        let safe_rows = rows.saturating_sub(2);

        let mut alive = Vec::with_capacity(self.bugs.len());

        for mut bug in self.bugs.drain(..) {
            let elapsed = now.duration_since(bug.birth);

            if elapsed >= bug.lifetime {
                // ── 虫子超时：恢复旧位置的原始字符 ──
                if let Some(ch) = bug.saved_char.take() {
                    Terminal::clear_at_to(dev, bug.x, bug.y, ch)?;
                    screen.restore_char(bug.x, bug.y);
                }
                // 丢弃
            } else {
                // ── 虫子还活着 ──

                // 1) 恢复旧位置的原始字符（飞走 → 文字还原）
                if let Some(ch) = bug.saved_char.take() {
                    Terminal::clear_at_to(dev, bug.x, bug.y, ch)?;
                    screen.restore_char(bug.x, bug.y);
                }

                // 2) 随机移动
                let mut rng = SimpleRng::new(
                    self.tick
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(bug.phase),
                );

                // ── 水平移动（边缘排斥 + 随机游走） ──
                let dx = if bug.x < 3 && bug.direction == -1 {
                    bug.direction = 1;
                    1
                } else if bug.x >= safe_cols && bug.direction == 1 {
                    bug.direction = -1;
                    -1
                } else {
                    match rng.next() % 10 {
                        0..=5 => bug.direction as i32,
                        6..=8 => 0,
                        _ => -bug.direction as i32,
                    }
                };

                let new_x = bug.x as i32 + dx;
                if new_x <= 0 {
                    bug.x = 1;
                    bug.direction = 1;
                } else if new_x as u16 >= cols {
                    bug.x = safe_cols.max(1);
                    bug.direction = -1;
                } else {
                    bug.x = new_x as u16;
                }

                // ── 垂直移动（范围 -2 ~ +2，边缘排斥） ──
                let dy = if bug.y < 2 {
                    1 + (rng.next() % 2) as i16
                } else if bug.y >= safe_rows {
                    -1 - (rng.next() % 2) as i16
                } else {
                    (rng.next() % 5) as i16 - 2
                };

                let new_y = bug.y as i16 + dy;
                if new_y <= 0 {
                    bug.y = 1;
                } else if new_y as u16 >= rows {
                    bug.y = safe_rows.max(1);
                } else {
                    bug.y = new_y as u16;
                }

                // 3) 保存新位置的原始字符（准备覆盖）
                let ch = screen.save_char(bug.x, bug.y);
                bug.saved_char = Some(ch);

                // 4) 在新位置绘制 Emoji
                Terminal::draw_bug_to(dev, bug.x, bug.y, bug.emoji)?;

                alive.push(bug);
            }
        }

        self.bugs = alive;
        Ok(())
    }

    /// 为刚创建的虫子（saved_char = None）保存字符并绘制 Emoji
    ///
    /// 仅对 trigger 后未保存过的新虫子执行，已有 saved_char 的跳过。
    /// 通常在 `trigger` 之后立即调用。
    pub fn draw_to<W: Write>(
        &mut self,
        dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        for bug in &mut self.bugs {
            if bug.saved_char.is_none() {
                let ch = screen.save_char(bug.x, bug.y);
                bug.saved_char = Some(ch);
                Terminal::draw_bug_to(dev, bug.x, bug.y, bug.emoji)?;
            }
        }
        Ok(())
    }

    /// 恢复所有虫子的原始字符（退出时调用）
    pub fn clear_all<W: Write>(
        &mut self,
        dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        for bug in self.bugs.drain(..) {
            if let Some(ch) = bug.saved_char {
                Terminal::clear_at_to(dev, bug.x, bug.y, ch)?;
                screen.restore_char(bug.x, bug.y);
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
