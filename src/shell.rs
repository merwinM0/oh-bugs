use std::io::{BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;

/// Shell 进程管理器
pub struct Shell {
    pub child: Child,
    stdin: ChildStdin,
    pub output_rx: std::sync::mpsc::Receiver<Vec<u8>>,
}

impl Shell {
    /// 启动一个新的 Shell 进程，并启动输出读取线程
    pub fn spawn() -> anyhow::Result<Self> {
        let mut child = Command::new("bash")
            .args(["--noediting", "--norc"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().expect("failed to get shell stdin");
        let stdout = child.stdout.take().expect("failed to get shell stdout");
        let stderr = child.stderr.take().expect("failed to get shell stderr");

        // 启动输出读取线程
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();

        // stdout 读取线程
        let tx1 = tx.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut temp = [0u8; 4096];
            loop {
                match reader.read(&mut temp) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = temp[..n].to_vec();
                        if tx1.send(data).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // stderr 读取线程
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut temp = [0u8; 4096];
            loop {
                match reader.read(&mut temp) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = temp[..n].to_vec();
                        if tx.send(data).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self { child, stdin, output_rx: rx })
    }

    /// 将用户输入写入 Shell 的 stdin
    pub fn write_input(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.stdin.write_all(data)?;
        self.stdin.flush()?;
        Ok(())
    }

    /// 尝试从输出 channel 接收数据（非阻塞）
    pub fn try_recv_output(&mut self) -> Result<Vec<u8>, std::sync::mpsc::TryRecvError> {
        self.output_rx.try_recv()
    }

    /// 检查子进程是否仍在运行
    pub fn is_running(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            _ => false,
        }
    }
}
