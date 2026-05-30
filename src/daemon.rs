//! 输出嗅探器主循环 — PTY 包装 + 屏幕缓冲区 + 虫子覆盖
//!
//! 1. 创建 PTY，启动用户 Shell
//! 2. 转发 stdin ↔ PTY master，转发 PTY master → stdout
//! 3. 同时将输出送入 ScreenBuffer 追踪屏幕内容
//! 4. 检测 error 关键字（仅检测输出，不检测输入）→ 触发虫子动画
//! 5. 虫子覆盖文字前保存原始字符，移走后恢复
//!
//! ## 输入回显过滤
//!
//! 用户输入通过 PTY 送达 Shell，Shell 会将输入回显到输出流中。
//! 为避免回显内容（如命令名、参数）中的 "error" 触发误报，
//! 本模块跟踪已发送的输入字节，在扫描前剥离匹配的回显前缀。

use crate::animation::BugManager;
use crate::config::Config;
use crate::screen::ScreenBuffer;
use crate::shell::Shell;
use crate::terminal::Terminal;
use crate::watcher::Watcher;

use anyhow::Context;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 全局退出标志
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// 运行测试模式（前台阻塞）
///
/// 只包装 PTY，不触发虫子。
/// 有 PTY 输出时，输出 10 组随机位置的 ScreenBuffer 字符到 stderr。
pub fn run_test() -> anyhow::Result<()> {
    let _term = Terminal::enter()?;

    let (cols, rows) = Terminal::size().unwrap_or((80, 24));
    let mut screen = ScreenBuffer::new(cols, rows);

    let mut shell = Shell::spawn().context("无法启动 Shell")?;
    let _ = shell.set_window_size(cols, rows);

    let (stdin_tx, stdin_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => { let _ = stdin_tx.send(Vec::new()); break; }
                Ok(_) => { if stdin_tx.send(vec![buf[0]]).is_err() { break; } }
                Err(_) => break,
            }
        }
    });

    let mut last_cols = cols;
    let mut last_rows = rows;
    let mut print_counter = 0u32;

    'main: loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            break 'main;
        }

        // 转发输入
        loop {
            match stdin_rx.try_recv() {
                Ok(data) => {
                    if data.is_empty() { break 'main; }
                    if let Err(e) = shell.write_input(&data) {
                        eprintln!("\r\n[oh-bugs] write error: {e}\r\n");
                        break 'main;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'main,
            }
        }

        // 转发输出 + 更新 ScreenBuffer
        let mut got_output = false;
        loop {
            match shell.try_recv_output() {
                Ok(data) => {
                    if let Err(e) = std::io::stdout().write_all(&data) {
                        eprintln!("\r\n[oh-bugs] stdout error: {e}\r\n");
                        break 'main;
                    }
                    std::io::stdout().flush()?;
                    screen.process_output(&data);
                    got_output = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'main,
            }
        }

        // 检查尺寸
        if let Ok((cols, rows)) = Terminal::size() {
            if cols != last_cols || rows != last_rows {
                last_cols = cols;
                last_rows = rows;
                let _ = shell.set_window_size(cols, rows);
                screen.resize(cols, rows);
            }
        }

        // 有输出时，打印 10 组随机位置
        if got_output {
            print_counter = print_counter.wrapping_add(1);
            // 每 3 次输出打印一次，避免刷屏太快
            if print_counter % 3 == 0 {
                let cols = screen.cols();
                let rows = screen.rows();
                let mut rng = SimpleRng::new(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as u64,
                );
                let max_x = cols.saturating_sub(2).max(1);
                for i in 0..10u32 {
                    let x = (rng.next() % max_x as u64) as u16;
                    let y = (rng.next() % rows.max(1) as u64) as u16;
                    let ch1 = screen.get_char(x, y);
                    let ch2 = screen.get_char(x + 1, y);
                    eprintln!("[TEST {}] pos ({},{}): char1={:?} char2={:?}", i + 1, x, y, ch1, ch2);
                }
                eprintln!("---");
            }
        }

        if !shell.is_running() {
            break 'main;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}

/// 运行嗅探器主循环（前台阻塞）
pub fn run() -> anyhow::Result<()> {
    let config = Config::load();

    // ── 进入 raw mode ──
    let _term = Terminal::enter()?;

    // ── 获取终端尺寸 ──
    let (cols, rows) = Terminal::size().unwrap_or((80, 24));
    let mut screen = ScreenBuffer::new(cols, rows);

    // ── 初始化嗅探器 ──
    let watcher = Watcher::new(&config.error_keywords)?;

    // ── 启动 Shell ──
    let mut shell = Shell::spawn().context("无法启动 Shell")?;
    let _ = shell.set_window_size(cols, rows);

    // ── stdin 读取线程 ──
    let (stdin_tx, stdin_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => {
                    let _ = stdin_tx.send(Vec::new());
                    break;
                }
                Ok(_) => {
                    if stdin_tx.send(vec![buf[0]]).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // ── 虫子管理器 ──
    let bug_manager = Arc::new(Mutex::new(BugManager::new(
        config.max_concurrent_bugs,
        cols,
        rows,
    )));

    let bug_lifetime = config.bug_lifetime_ms;

    // ── 输入回显跟踪 ──
    let mut pending_echo: VecDeque<u8> = VecDeque::with_capacity(1024);

    // ─── 主循环 ───
    let mut last_cols = cols;
    let mut last_rows = rows;

    'main: loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            break 'main;
        }

        // ── 1. 转发用户输入到 Shell ──
        loop {
            match stdin_rx.try_recv() {
                Ok(data) => {
                    if data.is_empty() {
                        break 'main;
                    }
                    if !data.is_empty() {
                        pending_echo.extend(&data);
                        while pending_echo.len() > 4096 {
                            pending_echo.pop_front();
                        }
                    }
                    if let Err(e) = shell.write_input(&data) {
                        eprintln!("\r\n[oh-bugs] write error: {e}\r\n");
                        break 'main;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'main,
            }
        }

        // ── 2. 读取 Shell 输出 → 转发 + 扫描 ──
        loop {
            match shell.try_recv_output() {
                Ok(data) => {
                    if let Err(e) = std::io::stdout().write_all(&data) {
                        eprintln!("\r\n[oh-bugs] stdout error: {e}\r\n");
                        break 'main;
                    }
                    std::io::stdout().flush()?;

                    screen.process_output(&data);

                    let (non_echo_data, _echo_consumed) = strip_echo(&data, &mut pending_echo);

                    if !non_echo_data.is_empty() {
                        let match_count = watcher.scan(non_echo_data);
                        if match_count > 0 {
                            let mut mgr = bug_manager.lock().unwrap();
                            mgr.trigger(
                                match_count,
                                Duration::from_millis(bug_lifetime),
                                &screen,
                            );
                            let _ = mgr.draw_to(&mut std::io::stdout(), &mut screen);
                            let _ = std::io::stdout().flush();
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'main,
            }
        }

        // 3. 检查终端尺寸变化
        if let Ok((cols, rows)) = Terminal::size() {
            if cols != last_cols || rows != last_rows {
                last_cols = cols;
                last_rows = rows;
                let _ = shell.set_window_size(cols, rows);
                screen.resize(cols, rows);
            }
        }

        // 4. 动画 tick
        {
            let mut mgr = bug_manager.lock().unwrap();
            let _ = mgr.update(&mut std::io::stdout(), &mut screen);
            let _ = std::io::stdout().flush();
        }

        // 5. 检查 shell 存活
        if !shell.is_running() {
            break 'main;
        }

        std::thread::sleep(Duration::from_millis(5));
    }

    // ─── 清理 ───
    {
        let mut mgr = bug_manager.lock().unwrap();
        let _ = mgr.clear_all(&mut std::io::stdout(), &mut screen);
        let _ = std::io::stdout().flush();
    }

    Ok(())
}

fn strip_echo<'a>(data: &'a [u8], pending: &mut VecDeque<u8>) -> (&'a [u8], usize) {
    let mut consumed = 0usize;
    for &b in data {
        match pending.front() {
            Some(&pe) if pe == b => { pending.pop_front(); consumed += 1; }
            _ => break,
        }
    }
    (&data[consumed..], consumed)
}

/// 简单 PRNG，供测试模式使用
struct SimpleRng { state: u64 }
impl SimpleRng {
    fn new(seed: u64) -> Self { Self { state: if seed == 0 { 1 } else { seed } } }
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.state >> 33
    }
}
