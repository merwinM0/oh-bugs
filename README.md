# oh！bugs！ — 屏幕漂浮虫子 · 输出嗅探器

## 1. 项目概述

Rust 编写的终端辅助工具。透明包装当前终端会话，实时监测输出流。当检测到 `error`（可配置）关键字时，**虫子 Emoji 像"漂浮"在屏幕上一样飞过**——短暂覆盖文字后飞走，原始文字自动恢复。不影响任何命令的执行，不修改终端的任何内容。

**核心设计理念：** 虫子是漂浮在屏幕上层的视觉特效，覆盖文字时保存原字符，飞走后写回恢复。启动即刻使用，零设置、零侵入。

## 2. CLI 命令

| 命令              | 效果                                     |
| ----------------- | ---------------------------------------- |
| `obugs --on`      | **启动**嗅探器（前台，包装当前终端）     |
| `obugs --off`     | **停止**嗅探器（从另一个终端执行）       |
| `obugs --status`  | 查看嗅探器运行状态                       |
| `obugs --help`    | 显示帮助信息                             |

### 2.1 典型工作流

```bash
# 在当前终端直接启动
obugs --on
# 🐛 oh-bugs! 嗅探器已启动 (PID 12345)
# 当前终端已包装 - 有 error 时自动放虫子

# 正常使用终端，你的 Shell 完全不变
ls -la
cargo build    # 输出中有 error → 虫子飘出！
git status

# 从另一个终端停止
obugs --off
# 🐛 oh-bugs! 嗅探器已停止
```

## 3. 核心原理：屏幕缓冲区 + 保存恢复

### 3.1 虫子不修改文字的原理

```
步骤 1: Shell 输出 "Error: not found"
         → 屏幕缓冲区记录每个位置的字符
         → 转发到真实终端显示

步骤 2: 检测到 "error" → 触发虫子
         → 从屏幕缓冲区读取 (x,y) 处的字符 'n'
         → 保存 'n'，绘制虫子 Emoji 🐝 覆盖

步骤 3: 虫子飞走
         → 从缓冲区读取已保存的 'n'
         → 写回终端：位置 (x,y) 恢复为 'n'

步骤 4: 虫子消失
         → 所有被覆盖位置全部恢复原文字
```

**结果：** 用户看到虫子飞过时压住了文字，飞走后文字完好如初。整个过程中终端内容没有被修改。

### 3.2 架构图

```
当前终端（raw mode）
     ↕
oh-bugs 主循环
├── [转发] stdin → PTY master
├── [转发] PTY master → stdout
├── [跟踪] PTY 输出 → 同步更新 ScreenBuffer (字符网格)
├── [嗅探] 扫描输出 → 匹配 error 关键字
├── [覆盖] 触发虫子 → 从 ScreenBuffer 保存字符 → 绘制 Emoji
└── [恢复] 虫子移动/消失 → 从 ScreenBuffer 取出字符 → 写回终端
     ↕ PTY master
Linux PTY Driver
     ↕ PTY slave
Shell (bash/zsh/fish)
```

### 3.3 ScreenBuffer 工作原理

ScreenBuffer 维护一个 `cols × rows` 的字符网格，通过解析 PTY 输出流来追踪终端内容：

| 输出内容             | ScreenBuffer 处理                                |
| -------------------- | ------------------------------------------------ |
| 可打印字符 (a-z, 0-9) | 写入网格当前光标位置，光标前进                    |
| `\n` / `\r`          | 换行/回车，必要时上滚                             |
| `\t`                 | 跳到下一个制表位                                  |
| `\x1b[<r>;<c>H`      | 光标定位 (CUP)                                   |
| `\x1b[<n>C` / `D`    | 光标前后移动                                      |
| `\x1b[K`             | 清除行                                           |
| `\x1b[J` / `\x1b[2J` | 清除屏幕                                          |
| UTF-8 多字节字符      | 完整解码后写入                                    |
| 颜色/SGR 序列         | 忽略（不影响字符位置）                            |

## 4. 虫子动画规则

| 属性     | 规则                                                  |
| -------- | ----------------------------------------------------- |
| 虫子池   | 🦟  🪰  🐝  🕷️  🦗                                    |
| 触发条件 | 每匹配到 1 个 "error" 关键字                          |
| 数量     | 每次触发随机生成 2~5 只                               |
| 出现位置 | 终端上半区的随机位置                                  |
| 移动方式 | 每 tick 水平漂移 1 列，随机上下浮动 1 行              |
| 生命周期 | 单只虫子存活 2~3 秒后消失（同时恢复覆盖的文字）       |
| 最大并发 | 屏幕同时最多显示 30 只虫子                            |
| 碰壁反弹 | 虫子遇到屏幕边缘会反弹方向                            |

## 5. 模块设计

```
src/
├── main.rs           # CLI 入口：--on / --off / --status
├── snooper.rs        # 生命周期管理（PID、信号）
├── daemon.rs         # 主循环（PTY 转发 + 屏幕缓冲 + 虫子覆盖）
├── shell.rs          # PTY Shell 进程管理
├── screen.rs         # 屏幕缓冲区（追踪字符、保存/恢复）
├── animation.rs      # 虫子动画引擎（保存→覆盖→恢复）
├── terminal.rs       # 终端控制（crossterm + 设备文件）
├── watcher.rs        # 关键字扫描
└── config.rs         # 配置加载
```

### 5.1 核心数据流

```
Shell 输出
   ↓
PTY master read   ──→  [1] 写入真实终端 stdout
                         [2] 送入 ScreenBuffer.process_output()
                         [3] 送入 Watcher.scan()
                              ↓ 匹配 error
                         [4] BugManager.trigger() 生成虫子
                         [5] BugManager.draw_to()
                              ├─ screen.save_char(x, y) 保存原字符
                              └─ Terminal.draw_bug_to() 绘制 Emoji

动画 tick (每 ~5ms)
   ↓
BugManager.update()
   ├─ 移走的虫子 → screen.restore_char() → 写回原字符
   └─ 存活的虫子 → 更新位置
BugManager.draw_to()
   ├─ screen.save_char() 保存新位置字符
   └─ Terminal.draw_bug_to() 绘制虫子

退出时
   ↓
BugManager.clear_all()
   └─ 恢复所有被覆盖的字符 → 写回终端
```

## 6. 技术选型

| 组件              | 选型               | 用途                                 |
| ----------------- | ------------------ | ------------------------------------ |
| 语言              | Rust               | 单二进制 (~3.6MB)                    |
| 终端控制          | crossterm          | raw mode、ANSI 控制                  |
| PTY 创建          | nix + libc         | posix_openpt、ioctl                  |
| 屏幕追踪          | 自制 ScreenBuffer  | 解析 ANSI 序列，维护字符网格          |
| 关键字匹配        | regex              | 大小写不敏感扫描                     |
| 配置解析          | serde + toml       | TOML 配置文件                        |

## 7. 安装与使用

### 7.1 编译
```bash
cargo build --release
# 二进制: target/release/oh-bugs
```

### 7.2 运行
```bash
# 当前终端直接启动
./target/release/oh-bugs --on
# 正常使用，有 error 时自动放虫子

# 从另一个终端停止
./target/release/oh-bugs --off
```

### 7.3 安装到 PATH
```bash
cargo install --path .
obugs --on
obugs --off
```

### 7.4 配置 `~/.config/obugs/config.toml`

```toml
error_keywords = ["error", "fail", "exception", "fatal"]
min_bugs = 2
max_bugs = 5
bug_lifetime_ms = 2500
refresh_rate_ms = 100
max_concurrent_bugs = 30
```

---

**oh-bugs! 让错误变得有趣一点 🐛**
