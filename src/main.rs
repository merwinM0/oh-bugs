mod animation;
mod config;
mod shell;
mod terminal;
mod watcher;

use animation::BugManager;
use config::Config;
use shell::Shell;
use terminal::Terminal;
use watcher::Watcher;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    // 加载配置
    let config = Config::load();

    // 进入终端原始模式
    let term = Terminal::enter()?;
    let (cols, rows) = Terminal::size()?;

    // 初始化虫子管理器
    let bug_manager = Arc::new(Mutex::new(BugManager::new(
        config.max_concurrent_bugs,
        cols,
        rows,
    )));

    // 初始化输出监视器
    let watcher = Watcher::new(&config.error_keywords)?;

    // --- 打印欢迎信息 ---
    // --- 启动 Shell（基于 PTY）---
    let mut shell = Shell::spawn()?;
    shell.set_window_size(cols, rows)?;

    println!("\r\n🐛 oh-bugs! 增强终端已启动 — 输入 exit 或 Ctrl+D 退出");
    println!("\r 🐚 Shell: {} | 虫子关键字: {:?}", shell.shell_name, config.error_keywords);
    println!("\r");
    std::io::stdout().flush()?;

    // 启动 stdin 读取线程（将用户输入字节发到 channel）
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
                Err(e) => {
                    eprintln!("\r\n[oh-bugs] stdin error: {}\r\n", e);
                    break;
                }
            }
        }
    });

    let bug_lifetime = config.bug_lifetime_ms;

    // 启动动画线程 + 窗口尺寸同步
    let bug_mgr_anim = bug_manager.clone();
    let refresh_rate = config.refresh_rate_ms;
    let _anim_handle = std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(refresh_rate));

        if let Ok((cols, rows)) = Terminal::size() {
            let mut mgr = bug_mgr_anim.lock().unwrap();
            mgr.resize(cols, rows);
            mgr.update();
            mgr.draw();
        }
    });

    // --- 主循环: 转发 stdin ↔ PTY master ↔ stdout ---
    let mut input_buffer = String::new();
    let mut last_cols = cols;
    let mut last_rows = rows;

    'main_loop: loop {
        // 1. 检查是否有用户输入
        loop {
            match stdin_rx.try_recv() {
                Ok(data) => {
                    if data.is_empty() {
                        break 'main_loop; // Ctrl+D
                    }

                    // 在输入缓冲区中累积检测 "exit"
                    if let Ok(text) = std::str::from_utf8(&data) {
                        input_buffer.push_str(text);
                        if input_buffer.contains("exit\r") || input_buffer.contains("exit\n") {
                            let _ = shell.write_input(b"exit\n");
                            break 'main_loop;
                        }
                        if text.contains('\r') || text.contains('\n') {
                            input_buffer.clear();
                        }
                    }

                    // 转发到 PTY master
                    if let Err(e) = shell.write_input(&data) {
                        eprintln!("\r\n[oh-bugs] write error: {}\r\n", e);
                        break 'main_loop;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'main_loop,
            }
        }

        // 2. 读取 Shell 输出（从 PTY master）
        loop {
            match shell.try_recv_output() {
                Ok(data) => {
                    // 写入真实终端
                    if let Err(e) = std::io::stdout().write_all(&data) {
                        eprintln!("\r\n[oh-bugs] stdout error: {}\r\n", e);
                        break 'main_loop;
                    }
                    std::io::stdout().flush()?;

                    // 扫描错误关键字
                    let match_count = watcher.scan(&data);
                    if match_count > 0 {
                        let mut mgr = bug_manager.lock().unwrap();
                        mgr.trigger(match_count, Duration::from_millis(bug_lifetime));
                        mgr.draw();
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'main_loop,
            }
        }

        // 3. 检查终端尺寸变化，同步到 PTY
        if let Ok((cols, rows)) = Terminal::size() {
            if cols != last_cols || rows != last_rows {
                last_cols = cols;
                last_rows = rows;
                let _ = shell.set_window_size(cols, rows);
            }
        }

        // 4. 检查 shell 是否还活着
        if !shell.is_running() {
            break 'main_loop;
        }

        std::thread::sleep(Duration::from_millis(5));
    }

    // --- 清理 ---
    {
        let mut mgr = bug_manager.lock().unwrap();
        mgr.clear_all();
    }

    drop(term);
    println!("\r\n🐛 oh-bugs! 已退出，再见！");

    Ok(())
}
