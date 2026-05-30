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

/// 运行嗅探器主循环（前台阻塞）
///
/// 在当前终端中：
/// 1. 进入 raw mode
/// 2. 创建 PTY + 启动用户 Shell
/// 3. 转发 I/O + 屏幕缓冲 + 虫子覆盖
/// 4. 退出时恢复所有被虫子覆盖的文字
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
    // 记录已发送到 Shell 但尚未在输出中看到的字节，
    // 用于从扫描中剥离回显内容，避免输入中的关键字触发误报。
    let mut pending_echo: VecDeque<u8> = VecDeque::with_capacity(1024);

    // ─── 主循环 ───
    let mut last_cols = cols;
    let mut last_rows = rows;

    'main: loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            break 'main;
        }

        // ── 1. 转发用户输入到 Shell ──
        //    仅转发，不扫描（输入不参与关键字检测）
        loop {
            match stdin_rx.try_recv() {
                Ok(data) => {
                    if data.is_empty() {
                        break 'main; // Ctrl+D
                    }
                    // 记录已发送的输入字节，供后续回显剥离使用
                    if !data.is_empty() {
                        pending_echo.extend(&data);
                        // 限制队列大小，防止无回显场景（如密码输入）无限增长
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

        // ── 2. 读取 Shell 输出 → 转发 + 扫描（仅扫描非回显数据） ──
        loop {
            match shell.try_recv_output() {
                Ok(data) => {
                    // 转发输出到真实终端（始终完整转发）
                    if let Err(e) = std::io::stdout().write_all(&data) {
                        eprintln!("\r\n[oh-bugs] stdout error: {e}\r\n");
                        break 'main;
                    }
                    std::io::stdout().flush()?;

                    // 同步更新屏幕缓冲区（始终完整处理）
                    screen.process_output(&data);

                    // ── 扫描前剥离输入回显 ──
                    // Shell 会将用户输入回显到输出流中。
                    // 我们通过匹配 pending_echo 来识别并跳过回显部分，
                    // 确保只扫描真正的程序输出。
                    let (non_echo_data, _echo_consumed) = strip_echo(&data, &mut pending_echo);

                    // 只对非回显数据进行关键字扫描
                    if !non_echo_data.is_empty() {
                        let match_count = watcher.scan(non_echo_data);
                        if match_count > 0 {
                            let mut mgr = bug_manager.lock().unwrap();
                            mgr.trigger(
                                match_count,
                                Duration::from_millis(bug_lifetime),
                                &screen,
                            );
                            // 触发后立即保存并绘制
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

        // 4. 动画 tick：update 内部完成恢复旧位置 → 移动 → 保存新位置 → 绘制
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

    // ─── 清理：恢复所有被虫子覆盖的文字 ───
    {
        let mut mgr = bug_manager.lock().unwrap();
        let _ = mgr.clear_all(&mut std::io::stdout(), &mut screen);
        let _ = std::io::stdout().flush();
    }

    Ok(())
}

/// 从输出数据中剥离与 pending_echo 匹配的回显前缀。
///
/// 返回 (未匹配的数据切片, 本次消费的 echo 字节数)。
///
/// Shell 通常逐字节或小批量回显用户输入。本函数从 pending_echo
/// 逐个字节消费，与输出数据的前缀匹配。无论是否完全匹配，不匹配
/// 之后的数据都视为非回显输出，交给 watcher 扫描。
///
/// 边界情况：
/// - 无回显（如密码输入）：pending_echo 堆积，输出不匹配 → 全部数据都会扫描（安全）
/// - 部分回显：只剥离匹配的前缀，剩余部分扫描（安全）
fn strip_echo<'a>(data: &'a [u8], pending: &mut VecDeque<u8>) -> (&'a [u8], usize) {
    let mut consumed = 0usize;

    for &b in data {
        match pending.front() {
            Some(&pe) if pe == b => {
                pending.pop_front();
                consumed += 1;
            }
            _ => break, // 不匹配 → 剩余数据视为非回显
        }
    }

    (&data[consumed..], consumed)
}
