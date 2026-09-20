use super::*;
use std::io::{BufRead, BufReader};
use std::process::Child;
use std::sync::{mpsc, Arc, Mutex};

pub(super) struct Tunnel {
    child: Child,
    urls: mpsc::Receiver<String>,
    log: Arc<Mutex<String>>,
    pub url: Option<String>,
}

impl Tunnel {
    pub fn start(port: u16, workspace: &str) -> Result<Self> {
        let mut command = Command::new("cloudflared");
        command.args([
            "tunnel",
            "--no-autoupdate",
            "--url",
            &format!("http://127.0.0.1:{port}"),
            "--http-host-header",
            &format!("herdr.{}.localhost:{port}", workspace_slug(workspace)),
        ]);
        Self::spawn(command)
            .context("Cannot start cloudflared. Install it on the host running this popup")
    }

    fn spawn(mut command: Command) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let stderr = child
            .stderr
            .take()
            .context("Missing cloudflared log pipe")?;
        let (sender, urls) = mpsc::channel();
        let log = Arc::new(Mutex::new(String::new()));
        let last_log = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(url) = quick_url(&line) {
                    let _ = sender.send(url);
                }
                *last_log.lock().unwrap() = line;
            }
        });
        Ok(Self {
            child,
            urls,
            log,
            url: None,
        })
    }

    pub fn poll(&mut self) -> Result<()> {
        for url in self.urls.try_iter() {
            self.url = Some(url);
        }
        if let Some(status) = self.child.try_wait()? {
            bail!(
                "cloudflared exited ({status}): {}",
                self.log.lock().unwrap()
            );
        }
        Ok(())
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn quick_url(line: &str) -> Option<String> {
    line.split_whitespace().find_map(|word| {
        let host = word
            .strip_prefix("https://")?
            .strip_suffix(".trycloudflare.com")?;
        if !host.is_empty()
            && !host.starts_with('-')
            && !host.ends_with('-')
            && host
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            Some(word.to_owned())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_quick_tunnel_urls() {
        assert_eq!(
            quick_url("INF | https://some-random-name.trycloudflare.com |"),
            Some("https://some-random-name.trycloudflare.com".into())
        );
        for line in [
            "https://trycloudflare.com",
            "http://a.trycloudflare.com",
            "https://a.trycloudflare.com.evil.test",
            "https://a.b.trycloudflare.com",
            "https://-a.trycloudflare.com",
        ] {
            assert_eq!(quick_url(line), None);
        }
    }

    #[test]
    fn reports_child_failure() {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 7"]);
        let mut tunnel = Tunnel::spawn(command).unwrap();
        tunnel.child.wait().unwrap();
        assert!(tunnel
            .poll()
            .unwrap_err()
            .to_string()
            .contains("cloudflared exited"));
    }

    #[test]
    fn captures_url_and_stops_child_on_drop() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "echo 'https://test-tunnel.trycloudflare.com' >&2; exec sleep 30",
        ]);
        let mut tunnel = Tunnel::spawn(command).unwrap();
        let pid = tunnel.child.id();
        let url = tunnel
            .urls
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert_eq!(url, "https://test-tunnel.trycloudflare.com");
        tunnel.poll().unwrap();
        drop(tunnel);
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }
}
