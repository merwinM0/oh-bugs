# oh！bugs！ — 屏幕漂浮虫子 · 输出嗅探器

## 1. 项目概述

Rust 编写的终端辅助工具。透明包装当前终端会话，实时监测输出流。当检测到 `error`（可配置）关键字时，**虫子 Emoji 像"漂浮"在屏幕上一样飞过**——短暂覆盖文字后飞走，原始文字自动恢复。不影响任何命令的执行，不修改终端的任何内容。

**核心设计理念：** 虫子是漂浮在屏幕上层的视觉特效，覆盖文字时保存原字符，飞走后写回恢复。启动即刻使用，零设置、零侵入。

**⚠️ 检测范围说明：** 关键字检测**仅针对 Shell/程序的输出**，**不检测用户输入**。当用户打字时，Shell 会将输入回显到输出流中。为避免回显内容（如命令名 `grep error`）触发误报，系统通过回显剥离机制自动过滤掉匹配的输入回显前缀，仅对程序实际输出进行关键字扫描。

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

虫子 Emoji（🦟🪰🐝🕷️🦗）在大多数终端中占 **2 列宽度**，
每只虫子覆盖 (x, y) 和 (x+1, y) 两个单元格。

```
步骤 1: Shell 输出 "Error: not found"
         → ScreenBuffer 记录每个位置的字符
         → 转发到真实终端显示

步骤 2: 检测到 "error" → 触发虫子
         → SavedCell::capture(screen, x, y) 读取 ScreenBuffer
         → 存入虫子自身的 saved_left / saved_right（备份）
         → 在 (x, y) 处绘制 2 列宽的虫子 Emoji 🐝

步骤 3: 虫子飞走
         → 从 saved_left.character 取出备份字符
         → Terminal::clear_at_to() 写回终端
         → 位置 (x,y) 和 (x+1,y) 原样恢复

步骤 4: 虫子消失
         → 所有虫子用自己体内的备份恢复原文字
```

**结果：** 用户看到虫子飞过时压住了文字，飞走后文字完好如初。整个过程中终端内容没有被修改。

### 3.2 架构图

```
当前终端（raw mode）
     ↕
oh-bugs 主循环
├── [转发] stdin → PTY master（仅转发，不扫描）
├── [转发] PTY master → stdout
├── [跟踪] PTY 输出 → 同步更新 ScreenBuffer (字符网格)
├── [回显剥离] PTY 输出 → 剥离匹配的输入回显前缀 → 非回显数据
├── [嗅探] 仅扫描非回显数据 → 匹配 error 关键字
├── [覆盖] 触发虫子 → 从 ScreenBuffer 读取字符 → 存入虫子自身的 saved_* → 绘制 Emoji
└── [恢复] 虫子移动/消失 → 从自身的 saved_* 取出字符 → 写回终端（始终用备份，不读 ScreenBuffer）
     ↕ PTY master
Linux PTY Driver
     ↕ PTY slave
Shell (bash/zsh/fish)
```

### 3.3 ScreenBuffer：纯文本追踪

ScreenBuffer 只追踪 PTY 输出的终端字符，**不感知虫子**。

```
Cell → 只存一个 char
ScreenBuffer → rows × cols 的二维字符网格
```

每个虫子通过自己的 `saved_left` / `saved_right` 自行管理覆盖/恢复：

```rust
struct Bug {
    saved_left:  SavedCell { character: char, x, y },
    saved_right: SavedCell { character: char, x, y },
}
```

| 数据流向                              | 说明                                                     |
| ------------------------------------- | -------------------------------------------------------- |
| PTY 输出 → `ScreenBuffer`            | 追踪每个位置的字符（虫子不干扰）                         |
| 虫子出现 → 读 `ScreenBuffer`          | `SavedCell::capture()` 读取当前位置的 `ch` 存入自己      |
| 虫子移动 → 写 `saved_*` 到终端       | `Terminal::clear_at_to()` — 用自己存的 character 恢复旧位 |
| 虫子移动 → 读 `ScreenBuffer` + 存自己 | 新位置存入 `saved_left` / `saved_right`                 |
| 虫子死亡 → 写 `saved_*` 到终端       | 用自己存的 character 恢复                                 |

PTY 输出解析（同前）：

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

> **设计要点：** 每只虫子通过 `saved_left` / `saved_right` 自己记住自己覆盖了什么内容。
> ScreenBuffer 只追踪 PTY 输出的终端字符，不感知虫子，完全解耦。

## 4. 虫子动画规则

| 属性        | 规则                                                                 |
| ----------- | -------------------------------------------------------------------- |
| 虫子池      | 🦟 🪰 🐝 🕷️ 🦗（每个 emoji 占 2 列宽度）                             |
| 覆盖宽度    | 每只虫子覆盖 (x, y) 和 (x+1, y) 两个单元格                           |
| 触发条件    | 每匹配到 1 个 "error" 关键字（仅检测输出，输入回显已自动剥离）        |
| 数量        | 每次触发随机生成 2~5 只                                               |
| 出现位置    | 终端上半区的随机位置，x ∈ [0, cols-2]；位置去重，避免两虫重叠        |
| 移动方式    | 水平/垂直各独立方向（direction_x/direction_y）：60% 继续 / 30% 停留 / 10% 反向 |
| 轨迹标识    | 每虫独立 `id` + 随机 `phase`，确保轨迹互不重叠                        |
| 保存策略    | 触发时 `SavedCell::capture()` 读取 ScreenBuffer 存入虫子自身 `saved_*` |
| 恢复策略    | 移动/死亡时始终用 `saved_*.character`（触发时刻备份），不读 ScreenBuffer |
| 光标保护    | 逐操作 save/restore 改为**批次级别**单次 save/restore：`draw_to` / `update` / `clear_all` 各自用 `\x1b7`…`\x1b8`（DECSC/DECRC）包裹整批，避免每个虫子每次操作独立 save/restore（3N 次/ tick）累积光标漂移 |
| 生命周期    | 每只虫子独立随机 2~3 秒（基于配置中心值 ±500ms），到期自动消失并恢复文字 |
| 最大并发    | 屏幕同时最多显示 30 只虫子                                            |
| 边缘处理    | 距边缘 <3 列时强制向中心方向走，避免堆积在屏幕边缘                  |
| 消失恢复    | 超时后无条件恢复两个单元格的备份字符，不会残留                        |

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
                              └─ 更新 cells[r][c].ch（纯文本，不感知虫子）
                         [3] 送入 strip_echo() 剥离输入回显前缀
                              ↓ 非回显数据
                         [4] 送入 Watcher.scan()
                              ↓ 匹配 error
                         [5] BugManager.trigger() 生成虫子
                         [6] BugManager.draw_to() — 初始化新虫子
                              ├─ \x1b7（DECSC 保存光标）
                              ├─ 遍历新虫子，逐个 CUP + 绘制 Emoji
                              └─ \x1b8（DECRC 恢复光标）

动画 tick (每 ~5ms)
   ↓
BugManager.update()
   ├─ \x1b7（DECSC 保存光标）—— 整批只存一次
   ├─ 遍历每只虫子：
   │   死亡：从 ScreenBuffer 实时读字符 → CUP + 写回终端
   │   存活：
   │     1. CUP + 写回字符（恢复旧位）
   │     2. 随机移动（更新 x, y）
   │     3. CUP + 绘制 Emoji（新位）
   └─ \x1b8（DECRC 恢复光标）—— 整批只恢复一次

退出时
   ↓
BugManager.clear_all()
   ├─ \x1b7（DECSC 保存光标）
   ├─ 遍历所有 bugs，逐个 CUP + 写回字符
   └─ \x1b8（DECRC 恢复光标）
```

> **核心思路：** 每只虫子 **自己记住自己覆盖了什么内容**（`saved_left` / `saved_right`），
> 移动时把 saved_* 恢复到旧位置，再保存新位置的内容；
> 死亡时把 saved_* 恢复到当前位置。
> ScreenBuffer 只追踪终端字符，不感知虫子，完全解耦。
> 
> **避免累积漂移：** 不采用逐操作 save/restore，而是每批次（`draw_to` / `update` / `clear_all`）
> 在开头做一次 `\x1b7`（DECSC 保存光标），在结尾做一次 `\x1b8`（DECRC 恢复光标）。
> 无论批次内有几只虫子，光标始终只存一次、恢复一次，
> 彻底消除逐虫 save/restore 不可靠导致的多触发累积错位。

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
