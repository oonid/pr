use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub enum Step {
    Reply {
        status: &'static str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    Stall {
        body: Vec<u8>,
        sent: usize,
        pause: Duration,
    },
    Truncated {
        body: Vec<u8>,
        sent: usize,
    },
    Trickle {
        body: Vec<u8>,
        chunks: usize,
        gap: Duration,
    },
}

pub fn ok(body: &[u8]) -> Step {
    Step::Reply {
        status: "200 OK",
        headers: Vec::new(),
        body: body.to_vec(),
    }
}

pub fn status(status: &'static str) -> Step {
    Step::Reply {
        status,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub authorization: Option<String>,
}

pub struct Server {
    pub base: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("requests").clone()
    }
}

pub fn serve(steps: Vec<Step>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for step in steps {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let seen = Arc::clone(&seen);
            std::thread::spawn(move || handle(stream, step, &seen));
        }
    });
    Server { base, requests }
}

fn handle(mut stream: std::net::TcpStream, step: Step, seen: &Mutex<Vec<Request>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    let mut authorization = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value.trim().to_string());
            }
        }
    }
    seen.lock()
        .expect("requests")
        .push(Request { authorization });
    let _ = respond(&mut stream, step);
}

fn respond(stream: &mut std::net::TcpStream, step: Step) -> std::io::Result<()> {
    let head = |stream: &mut std::net::TcpStream,
                status: &str,
                headers: &[(String, String)],
                length: usize|
     -> std::io::Result<()> {
        write!(
            stream,
            "HTTP/1.1 {}\r\nContent-Length: {}\r\n",
            status, length
        )?;
        for (name, value) in headers {
            write!(stream, "{}: {}\r\n", name, value)?;
        }
        write!(stream, "Connection: close\r\n\r\n")
    };
    match step {
        Step::Reply {
            status,
            headers,
            body,
        } => {
            head(stream, status, &headers, body.len())?;
            stream.write_all(&body)
        }
        Step::Stall { body, sent, pause } => {
            head(stream, "200 OK", &[], body.len())?;
            stream.write_all(&body[..sent])?;
            stream.flush()?;
            std::thread::sleep(pause);
            stream.write_all(&body[sent..])
        }
        Step::Truncated { body, sent } => {
            head(stream, "200 OK", &[], body.len())?;
            stream.write_all(&body[..sent])
        }
        Step::Trickle { body, chunks, gap } => {
            head(stream, "200 OK", &[], body.len())?;
            let size = body.len().div_ceil(chunks);
            for chunk in body.chunks(size) {
                stream.write_all(chunk)?;
                stream.flush()?;
                std::thread::sleep(gap);
            }
            Ok(())
        }
    }
}
