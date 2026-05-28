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
    let welcome = format!(
        "\r\n🐛 oh-bugs! 增强终端已启动 — 输入 exit 或 Ctrl+D 退出\r\n\
         \r 配置: ~/.config/obugs/config.toml\r\n\
         \r 虫子将在检测到关键字时出现: {:?}\r\n\r\n",
        config.error_keywords
    );
    print!("{}", welcome);
    std::io::stdout().flush()?;

    // --- 启动 Shell ---
    let mut shell = Shell::spawn()?;

    // 启动 stdin 读取线程（将用户输入发到 channel）
    let (stdin_tx, stdin_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1]; // 一次读取一个字节以支持原始模式
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => {
                    // EOF (Ctrl+D)
                    let _ = stdin_tx.send(Vec::new());
                    break;
                }
                Ok(_) => {
                    let data = vec![buf[0]];
                    if stdin_tx.send(data).is_err() {
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

    // 启动动画线程
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

    // --- 主循环: 转发 stdin -> shell, shell stdout -> stdout ---
    // 收集累积的输入行，用于检测 "exit" 命令
    let mut input_buffer = String::new();

    'main_loop: loop {
        // 1. 检查是否有用户输入
        loop {
            match stdin_rx.try_recv() {
                Ok(data) => {
                    if data.is_empty() {
                        // EOF / Ctrl+D
                        break 'main_loop;
                    }

                    // 将输入累计到缓冲区检测 exit 命令
                    if let Ok(text) = std::str::from_utf8(&data) {
                        input_buffer.push_str(text);

                        // 检测是否输入了 "exit" 后跟回车
                        if input_buffer.contains("exit\r") || input_buffer.contains("exit\n") {
                            // 发送 exit 到 shell
                            let _ = shell.write_input(b"exit\n");
                            break 'main_loop;
                        }

                        // 如果遇到回车，清空输入缓冲区（新一行开始）
                        if text.contains('\r') || text.contains('\n') {
                            input_buffer.clear();
                        }
                    }

                    // 将输入转发给 Shell
                    if let Err(e) = shell.write_input(&data) {
                        eprintln!("\r\n[oh-bugs] shell write error: {}\r\n", e);
                        break 'main_loop;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    break; // 没有输入了
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    break 'main_loop;
                }
            }
        }

        // 2. 读取 Shell 输出
        loop {
            match shell.try_recv_output() {
                Ok(data) => {
                    // 写入终端
                    if let Err(e) = std::io::stdout().write_all(&data) {
                        eprintln!("\r\n[oh-bugs] stdout error: {}\r\n", e);
                        break 'main_loop;
                    }
                    if let Err(e) = std::io::stdout().flush() {
                        eprintln!("\r\n[oh-bugs] stdout flush error: {}\r\n", e);
                        break 'main_loop;
                    }

                    // 扫描错误关键字
                    let match_count = watcher.scan(&data);
                    if match_count > 0 {
                        let mut mgr = bug_manager.lock().unwrap();
                        mgr.trigger(
                            match_count,
                            Duration::from_millis(bug_lifetime),
                        );
                        // 立即绘制一波
                        mgr.draw();
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    break; // 没有更多输出
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    break 'main_loop;
                }
            }
        }

        // 3. 检查 shell 是否还活着
        if !shell.is_running() {
            break 'main_loop;
        }

        // 短暂休眠避免忙等待
        std::thread::sleep(Duration::from_millis(5));
    }

    // --- 清理 ---
    // 清除所有虫子
    {
        let mut mgr = bug_manager.lock().unwrap();
        mgr.clear_all();
    }

    // 恢复终端（Terminal::drop 会自动处理）
    drop(term);

    println!("\r\n🐛 oh-bugs! 已退出，再见！");

    Ok(())
}
