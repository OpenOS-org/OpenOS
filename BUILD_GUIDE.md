# OpenOS 构建与使用指南

从零开始构建并运行 OpenOS 微内核操作系统。

## 目录

1. [环境准备](#1-环境准备)
2. [获取源码](#2-获取源码)
3. [构建内核](#3-构建内核)
4. [构建用户程序](#4-构建用户程序)
5. [创建 Initrd](#5-创建-initrd)
6. [构建磁盘镜像](#6-构建磁盘镜像)
7. [运行系统](#7-运行系统)
8. [使用 Shell](#8-使用-shell)
9. [文件系统操作](#9-文件系统操作)
10. [运行其他程序](#10-运行其他程序)
11. [运行测试](#11-运行测试)
12. [常见问题](#12-常见问题)

---

## 1. 环境准备

### 必需软件

| 软件 | 用途 | 安装命令 (Ubuntu/Debian) |
|------|------|--------------------------|
| Rust (nightly) | 编译内核和用户程序 | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| NASM | 汇编用户程序 | `sudo apt install nasm` |
| QEMU | 虚拟机 | `sudo apt install qemu-system-x86` |
| binutils | 链接器 ld | `sudo apt install binutils` |
| Git | 版本控制 | `sudo apt install git` |

### 安装 Rust Nightly

```bash
# 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# 安装 nightly 工具链
rustup install nightly

# 添加 rust-src 组件（内核编译需要）
rustup component add rust-src --toolchain nightly

# 设置项目使用 nightly
rustup override set nightly
```

### 验证环境

```bash
rustc --version    # 应显示 nightly 版本
nasm --version     # NASM version 2.x
qemu-system-x86_64 --version  # QEMU emulator version 8.x
ld --version       # GNU ld
git --version      # git version 2.x
```

---

## 2. 获取源码

```bash
# 克隆仓库
git clone https://github.com/LemonStudio-hub/open-os.git
cd open-os

# 查看项目结构
ls -la
# 应看到: kernel/ sdk/ user/ tools/ Cargo.toml Makefile CLAUDE.md
```

### 项目结构速览

```
openos/
  kernel/          # 内核源码 (Rust, bare-metal)
    src/
      main.rs      # 内核入口
      arch/        # x86_64 架构代码 (GDT, IDT, SYSCALL, APIC)
      drivers/     # 硬件驱动 (串口, VGA, 键盘, VirtIO, PCI)
      fs/          # 文件系统 (VFS, ramfs, ext2, procfs, devfs)
      memory/      # 内存管理 (堆, 页表, 帧分配器)
      net/         # 网络协议栈 (TCP, UDP, IP, ARP, DHCP, DNS)
      syscall/     # 系统调用 (89个)
      task/        # 任务调度 (SMP轮转, 信号, 定时器)
  sdk/             # 用户空间 SDK (18个模块)
  user/            # 用户程序
    coreutils/     # 98+ 类Linux命令
    shell_rs/      # 交互式 Shell
    fstest.asm     # 文件系统测试
    hello_rs/      # Hello World
    ...
  tools/
    mkinitrd/      # Initrd 构建工具
```

---

## 3. 构建内核

### 编译内核

```bash
cargo build -p openos-kernel \
  --target x86_64-unknown-none \
  -Zbuild-std=core,compiler_builtins,alloc \
  -Zbuild-std-features=compiler-builtins-mem \
  --release
```

编译成功后会生成：
```
target/x86_64-unknown-none/release/openos-kernel
```

### 验证内核二进制

```bash
file target/x86_64-unknown-none/release/openos-kernel
# 输出: ELF 64-bit LSB pie executable, x86-64, version 1 (SYSV)
```

---

## 4. 构建用户程序

OpenOS 有两种用户程序：NASM 汇编程序和 Rust 程序。

### 构建汇编程序

```bash
# 控制台服务
nasm -f elf64 user/console_svc.asm -o target/debug/console_svc.o
ld -static -o target/release/console_svc.elf target/debug/console_svc.o

# 文件系统测试
nasm -f elf64 user/fstest.asm -o target/debug/fstest.o
ld -static -o target/release/fstest.elf target/debug/fstest.o
```

### 构建 Rust 程序

```bash
# Shell (最重要的用户程序)
cargo build -p shell-rs \
  --target x86_64-unknown-none \
  -Zbuild-std=core,compiler_builtins,alloc \
  -Zbuild-std-features=compiler-builtins-mem \
  --release
cp target/x86_64-unknown-none/release/shell-rs target/release/shell_rs.elf

# Hello World
cargo build -p hello-rs \
  --target x86_64-unknown-none \
  -Zbuild-std=core,compiler_builtins,alloc \
  -Zbuild-std-features=compiler-builtins-mem \
  --release
cp target/x86_64-unknown-none/release/hello target/release/hello_rs.elf

# 构建所有 coreutils (ls, cat, cp, rm, echo, ...)
cargo build -p coreutils \
  --target x86_64-unknown-none \
  -Zbuild-std=core,compiler_builtins,alloc \
  -Zbuild-std-features=compiler-builtins-mem \
  --release

# 复制 coreutils 二进制文件
for cmd in ls cat echo pwd touch rm cp mv head tail wc grep \
           sort uniq rev hexdump hostname uname uptime ps date \
           whoami id clear env which du df chmod ln mkdir rmdir \
           find diff cut tr paste fold expand unexpand od strings \
           file stat realpath readlink basename dirname sleep \
           yes seq true_cmd false_cmd tee; do
  cp target/x86_64-unknown-none/release/$cmd target/release/$cmd.elf 2>/dev/null || true
done

# 剥离调试符号（减小体积）
strip target/release/*.elf 2>/dev/null || true
```

---

## 5. 创建 Initrd

Initrd（初始 RAM 磁盘）是一个包含所有用户程序的归档文件。

### 使用 mkinitrd 工具

```bash
cargo run -p mkinitrd --target x86_64-unknown-linux-gnu --release -- \
  target/release/initrd.img \
  shell_rs.elf=target/release/shell_rs.elf \
  fstest.elf=target/release/fstest.elf \
  console_svc.elf=target/release/console_svc.elf \
  hello_rs.elf=target/release/hello_rs.elf \
  ls.elf=target/release/ls.elf \
  cat.elf=target/release/cat.elf \
  echo.elf=target/release/echo.elf \
  pwd.elf=target/release/pwd.elf \
  touch.elf=target/release/touch.elf \
  rm.elf=target/release/rm.elf \
  cp.elf=target/release/cp.elf \
  mv.elf=target/release/mv.elf \
  mkdir.elf=target/release/mkdir.elf \
  rmdir.elf=target/release/rmdir.elf \
  stat.elf=target/release/stat.elf
```

格式：`目标名.elf=源文件路径`。`目标名` 是内核查找程序时使用的名字。

---

## 6. 构建磁盘镜像

将内核 + Initrd 打包为可启动的 BIOS 磁盘镜像：

```bash
cargo run -p openos --target x86_64-unknown-linux-gnu --release -- \
  target/x86_64-unknown-none/release/openos-kernel \
  target/release/openos-bios.img \
  target/release/initrd.img
```

成功后生成：
```
Creating BIOS disk image...
  Kernel: target/x86_64-unknown-none/release/openos-kernel
  Output: target/release/openos-bios.img
  Ramdisk: target/release/initrd.img
Done: target/release/openos-bios.img
```

---

## 7. 运行系统

### 串口模式（推荐，文本输出）

```bash
qemu-system-x86_64 \
  -drive format=raw,file=target/release/openos-bios.img \
  -serial stdio \
  -display none
```

启动后会看到完整的启动日志，最后出现 Shell 提示符：
```
=================================
  OpenOS Microkernel v0.3.0
=================================

[OK] Ramfs initialized
[OK] IPC subsystem initialized
[VFS] Mounted filesystem at '/'
[...] Launching shell (user-space)
[OK] ELF loaded: entry=0xa25b, stack=0x800000000000

OpenOS Shell v0.6 (Rust)
Type 'help' for available commands.
[0] / $
```

`[0] / $ ` 是 Shell 提示符：`[退出码] 当前目录 $`

### 图形模式

```bash
qemu-system-x86_64 \
  -drive format=raw,file=target/release/openos-bios.img \
  -serial stdio
```

### 使用 Makefile

项目提供了 Makefile 简化构建流程：

```bash
make build       # 构建内核 + initrd + 磁盘镜像（debug 模式）
make release     # 构建优化版本
make run         # 构建并运行
make run-gui     # 构建并用图形模式运行
make debug       # 构建并用 GDB 调试
make test        # 运行内核单元测试
make check       # 运行格式 + 代码检查 + 构建
make clean       # 清理构建产物
```

---

## 8. 使用 Shell

### 基本操作

Shell 启动后会显示提示符 `[0] / $`。输入命令后按 **Enter** 执行。

```
[0] / $ help          # 显示帮助信息
[0] / $ ls            # 列出当前目录文件
[0] / $ pwd           # 显示当前路径
[0] / $ cd /          # 切换到根目录
[0] / $ echo hello    # 输出文本
```

### 所有可用命令

| 类别 | 命令 | 功能 |
|------|------|------|
| **文件操作** | `ls [path]` | 列出目录内容 |
| | `cat <file>` | 显示文件内容 |
| | `cp <src> <dst>` | 复制文件 |
| | `mv <src> <dst>` | 移动/重命名文件 |
| | `rm <file>` | 删除文件 |
| | `touch <file>` | 创建空文件 |
| | `stat <file>` | 显示文件元数据 |
| | `mkdir <dir>` | 创建目录 |
| | `rmdir <dir>` | 删除空目录 |
| | `chmod <mode> <file>` | 更改文件权限 |
| **导航** | `cd [dir]` | 切换目录 (`cd -`, `cd ~`) |
| | `pwd` | 显示当前路径 |
| **环境变量** | `env` | 显示所有变量 |
| | `export K=V` | 设置变量 |
| | `unset <key>` | 删除变量 |
| | `echo $VAR` | 展开变量 |
| **进程** | `run <elf>` | 运行程序 |
| | `ps` | 列进程 |
| | `exit` | 退出 Shell |
| **别名** | `alias` | 列别名 |
| | `alias name='val'` | 创建别名 |
| | `unalias <name>` | 删除别名 |
| **文本工具** | `head <file>` | 显示文件开头 |
| | `tail <file>` | 显示文件结尾 |
| | `wc <file>` | 统计行/词/字节 |
| | `grep <pat> <file>` | 搜索文本 |
| | `sort <file>` | 排序 |
| | `uniq <file>` | 去重 |
| **系统信息** | `uname` | 系统名称 |
| | `hostname` | 主机名 |
| | `date` | 日期时间 |
| | `uptime` | 运行时间 |
| | `whoami` | 当前用户 |
| | `id` | 用户ID |
| | `df` | 磁盘使用 |
| | `du` | 目录大小 |
| | `clear` | 清屏 |
| **其他** | `true` | 返回 0 |
| | `false` | 返回 1 |
| | `yes [msg]` | 重复输出 |
| | `seq [n]` | 生成序列 |
| | `sleep <n>` | 等待 n 秒 |
| | `basename <path>` | 路径基名 |
| | `dirname <path>` | 目录名 |
| | `which <cmd>` | 查找命令路径 |

---

## 9. 文件系统操作

### 创建和查看文件

```
[0] / $ touch /hello.txt        # 创建空文件
[0] / $ ls /                    # 列出根目录
hello.txt

[0] / $ stat /hello.txt         # 查看文件信息
size=0

[0] / $ echo "Hello OpenOS"     # 输出文本
Hello OpenOS
```

### 目录操作

```
[0] / $ mkdir /mydir            # 创建目录
[0] / $ ls /
mydir/
hello.txt

[0] / $ cd /mydir               # 进入目录
[0] /mydir $ pwd
/mydir

[0] /mydir $ touch newfile      # 在新目录中创建文件
[0] /mydir $ ls
newfile

[0] /mydir $ cd ..              # 返回上级目录
[0] / $ rmdir /mydir            # 删除空目录（需先删除其中的文件）
rm: cannot remove directory
[0] / $ rm /mydir/newfile       # 先删除文件
[0] / $ rmdir /mydir            # 再删除目录
[0] / $ ls /
hello.txt
```

### 快速验证文件系统

运行内置的文件系统测试：

```
[0] / $ run fstest.elf
=== Filesystem Test ===
  PASS: open/create
  PASS: write
  PASS: re-open
  PASS: read
  PASS: stat
  PASS: mkdir

All tests complete.
```

---

## 10. 运行其他程序

### 查看可用程序

Shell 只能运行 Initrd 中包含的程序。查看可用的程序：

```
[0] / $ ls /ram    # 如果 ramfs 挂载了程序
```

### 运行程序

```
# 格式: run <程序名.elf>
[0] / $ run hello_rs.elf          # Hello World
[0] / $ run console_svc.elf       # 控制台服务
[0] / $ run fstest.elf            # 文件系统测试
[0] / $ run uname.elf             # 系统信息
[0] / $ run date.elf              # 显示日期
[0] / $ run whoami.elf            # 显示用户
```

### 常用程序及预期输出

| 程序 | 输出 |
|------|------|
| `hello_rs.elf` | "Hello from user-space!" |
| `uname.elf` | "OpenOS" |
| `whoami.elf` | "root" |
| `date.elf` | 模拟日期（固定值） |
| `pwd.elf` | "/" |
| `ls.elf` | 文件列表（可能需要参数） |
| `true_cmd.elf` |（无输出，退出码0） |
| `false_cmd.elf` |（无输出，退出码1） |
| `yes.elf` | 无限输出 "y"（Ctrl+C 停止，但可能不支持） |

---

## 11. 运行测试

### 内核单元测试

```bash
cargo test -p openos-kernel --target x86_64-unknown-linux-gnu
```

预期输出：
```
running 1301 tests
test result: ok. 1278 passed; 0 failed; 23 ignored
```

### 文件系统集成测试（QEMU）

```bash
# 修改 kernel/src/main.rs 第 246 行，将程序名改为 fstest.elf
# 然后构建并运行：

cargo build -p openos-kernel --target x86_64-unknown-none \
  -Zbuild-std=core,compiler_builtins,alloc \
  -Zbuild-std-features=compiler-builtins-mem --release

# 构建仅含 fstest.elf 的 initrd
cargo run -p mkinitrd --target x86_64-unknown-linux-gnu --release -- \
  target/release/initrd.img fstest.elf=target/release/fstest.elf

# 构建磁盘镜像
cargo run -p openos --target x86_64-unknown-linux-gnu --release -- \
  target/x86_64-unknown-none/release/openos-kernel \
  /tmp/test-boot.img target/release/initrd.img

# 运行并捕获输出
timeout 10 qemu-system-x86_64 -drive format=raw,file=/tmp/test-boot.img \
  -serial file:/tmp/test-output.log -display none -no-reboot 2>/dev/null

# 查看结果
grep -E "PASS|FAIL|All tests" /tmp/test-output.log
```

### 代码质量检查

```bash
make check    # 格式 + 裁剪 + 构建
make lint     # clippy 严格检查
make fmt      # 格式检查
```

---

## 12. 常见问题

### Q: 构建失败 "error: could not compile `openos-kernel`"

**A:** 确保使用 nightly Rust 工具链：
```bash
rustup override set nightly
rustup component add rust-src --toolchain nightly
```

### Q: QEMU 提示 "Failed to get write lock"

**A:** 有 QEMU 进程仍在运行，杀掉它：
```bash
pkill -9 qemu
rm -f target/release/openos-bios.img.lock
```

### Q: Shell 启动后立即崩溃

**A:** 检查 Initrd 中是否包含 `shell_rs.elf`。确认 initrd 构建时正确指定了路径：
```bash
cargo run -p mkinitrd --target x86_64-unknown-linux-gnu --release -- \
  target/release/initrd.img \
  shell_rs.elf=target/release/shell_rs.elf
```

### Q: 某些程序崩溃 "DOUBLE FAULT" 或 "PAGE FAULT"

**A:** 确保使用的是 release 版本（已经修复了栈大小问题，用户栈为 16KB）。

### Q: 如何添加新的用户程序到系统中？

**A:** 
1. 编写程序（Rust 或 NASM 汇编）
2. 编译为 ELF 文件
3. 添加到 Initrd 中
4. 通过 Shell 的 `run` 命令运行

### Q: 系统内存不足

**A:** ramfs 限制为 32 个文件和 64KB 总存储。删除不需要的文件可释放空间。

### Q: 网络不工作

**A:** 默认 QEMU 配置没有 VirtIO-Net 设备。如需网络，需要：
1. 添加 `-netdev user,id=n0 -device virtio-net-pci,netdev=n0` 到 QEMU 参数
2. 确保内核编译时包含网络支持

---

## 快速参考卡片

```bash
# === 一键构建 ===
make release    # 构建所有（内核 + 用户程序 + initrd + 磁盘镜像）

# === 运行 ===
make run-release  # 启动 QEMU（串口模式）

# === 测试 ===
make test         # 运行内核单元测试

# === Shell 命令 ===
ls /              # 列出根目录
touch /test.txt   # 创建文件
mkdir /dir        # 创建目录
run fstest.elf    # 运行测试程序
exit              # 退出 Shell

# === 调试 ===
make debug        # QEMU + GDB
```

---

> **文档版本**: 2026-07-27 · **内核测试**: 1278 通过 · **用户程序**: 61+ 可用
