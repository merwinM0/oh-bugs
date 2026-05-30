//! oh-bugs! — 输出嗅探器（虫子漂浮覆盖模式）
//!
//! 透明包装当前终端，实时嗅探输出中的错误关键字。
//! 检测到 error 时释放"漂浮在屏幕上"的虫子 Emoji，
//! 虫子覆盖文字 → 飞走后自动恢复原文字。
//!
//! ## CLI
//!
//! | 命令              | 效果                                   |
//! | ----------------- | -------------------------------------- |
//! | `obugs --on`      | 启动嗅探器（前台，包装当前终端）       |
//! | `obugs --off`     | 停止嗅探器（从另一终端执行）           |
//! | `obugs --status`  | 查看运行状态                           |
//! | `obugs --test`    | 测试模式（不写终端，只输出调试信息）   |
//! | `obugs --help`    | 显示帮助信息                           |

mod animation;
mod config;
mod daemon;
mod screen;
mod shell;
mod snooper;
mod terminal;
mod watcher;

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");

    match cmd {
        "--on" | "on" | "-o" => {
            if let Err(e) = snooper::start_snooper() {
                eprintln!("⚠️  嗅探器退出: {e}");
                std::process::exit(1);
            }
        }
        "--off" | "off" => {
            if let Err(e) = snooper::stop_snooper() {
                eprintln!("⚠️  停止嗅探器失败: {e}");
                std::process::exit(1);
            }
        }
        "--status" | "status" | "-s" => {
            if let Err(e) = snooper::show_status() {
                eprintln!("⚠️  查询失败: {e}");
                std::process::exit(1);
            }
        }
        "--test" | "test" | "-t" => {
            if let Err(e) = snooper::start_test_snooper() {
                eprintln!("⚠️  测试退出: {e}");
                std::process::exit(1);
            }
        }
        "--help" | "help" | "-h" | "" => {
            print_help();
        }
        _ => {
            eprintln!("⚠️  未知命令: {}", args.get(1).unwrap_or(&"".to_string()));
            eprintln!();
            print_help();
            std::process::exit(1);
        }
    }
}

fn print_help() {
    eprintln!(r#"🐛 oh-bugs! — 屏幕漂浮虫子嗅探器（Output Snooper）
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

透明包装当前终端，实时检测输出中的错误关键字。
检测到 error 时，虫子 Emoji "漂浮"在屏幕上——
覆盖文字后飞走，原始文字自动恢复。

零侵入：不创建新 Shell，不改任何配置。

用法:
  obugs --on       启动嗅探器（当前终端直接使用）
  obugs --off      停止嗅探器（从另一个终端执行）
  obugs --status   查看嗅探器运行状态
  obugs --test     测试模式（不写终端，输出每个虫子占据位置的字符）
  obugs --help     显示此帮助信息

工作流程:
  obugs --on          # 包装当前终端，立即开始嗅探
  # 正常使用终端      # 有 error 时虫子飘出
  obugs --off         # 从另一个终端执行，停止

测试模式:
  obugs --test        # 启动测试模式
  # 正常使用终端      # 每有输出时，stderr 输出 10 组随机位置的字符
  Ctrl+D 或 obugs --off 退出

配置: ~/.config/obugs/config.toml
"#);
}
