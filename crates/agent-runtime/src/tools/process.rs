use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Debug)]
pub struct LimitedChildOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug)]
struct LimitedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

pub async fn read_limited_child_output(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    output_limit_bytes: usize,
) -> anyhow::Result<LimitedChildOutput> {
    let stdout_future = read_limited_stream(stdout, output_limit_bytes);
    let stderr_future = read_limited_stream(stderr, output_limit_bytes);
    tokio::pin!(stdout_future);
    tokio::pin!(stderr_future);

    let mut stdout_output: Option<LimitedOutput> = None;
    let mut stderr_output: Option<LimitedOutput> = None;

    while stdout_output.is_none() || stderr_output.is_none() {
        tokio::select! {
            output = &mut stdout_future, if stdout_output.is_none() => {
                stdout_output = Some(output?);
            }
            output = &mut stderr_future, if stderr_output.is_none() => {
                stderr_output = Some(output?);
            }
        }

        let stdout_truncated = stdout_output
            .as_ref()
            .is_some_and(|output| output.truncated);
        let stderr_truncated = stderr_output
            .as_ref()
            .is_some_and(|output| output.truncated);
        if stdout_truncated || stderr_truncated {
            return Ok(LimitedChildOutput {
                stdout: stdout_output.map(|output| output.bytes).unwrap_or_default(),
                stderr: stderr_output.map(|output| output.bytes).unwrap_or_default(),
                stdout_truncated,
                stderr_truncated,
            });
        }
    }

    let stdout = stdout_output.expect("stdout output should be captured");
    let stderr = stderr_output.expect("stderr output should be captured");
    Ok(LimitedChildOutput {
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    })
}

async fn read_limited_stream(
    mut stream: impl AsyncRead + Unpin,
    output_limit_bytes: usize,
) -> anyhow::Result<LimitedOutput> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let hard_limit = output_limit_bytes.saturating_add(1);

    loop {
        let remaining = hard_limit.saturating_sub(bytes.len());
        if remaining == 0 {
            return Ok(LimitedOutput {
                bytes,
                truncated: true,
            });
        }

        let read_len = remaining.min(buffer.len());
        let read = stream.read(&mut buffer[..read_len]).await?;
        if read == 0 {
            return Ok(LimitedOutput {
                bytes,
                truncated: false,
            });
        }

        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > output_limit_bytes {
            bytes.truncate(output_limit_bytes);
            return Ok(LimitedOutput {
                bytes,
                truncated: true,
            });
        }
    }
}
