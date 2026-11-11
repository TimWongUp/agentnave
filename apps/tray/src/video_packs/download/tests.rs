use super::*;

#[test]
fn streaming_tracks_bytes_and_cancelled_jobs_ignore_late_updates() {
    let root = tempfile::tempdir().unwrap();
    let key = (root.path().to_owned(), "272".to_owned());
    let cancel = Arc::new(AtomicBool::new(false));
    jobs().insert(
        key.clone(),
        Job {
            cancel: cancel.clone(),
            state: json!({"state": "downloading"}),
        },
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 131072\r\nConnection: close\r\n\r\n")
            .unwrap();
        socket.write_all(&vec![42; 131072]).unwrap();
    });
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut output = tempfile::tempfile().unwrap();
    transfer(
        &client,
        &format!("http://{address}/pack"),
        &mut output,
        &key,
        &cancel,
    )
    .unwrap();
    server.join().unwrap();
    assert_eq!(output.metadata().unwrap().len(), 131072);
    assert_eq!(jobs()[&key].state["downloaded"], 131072);
    assert_eq!(jobs()[&key].state["total"], 131072);
    cancel_locked(&mut jobs(), &key);
    assert!(check_cancel(&cancel).is_err());
    let replacement = Arc::new(AtomicBool::new(false));
    jobs().insert(
        key.clone(),
        Job {
            cancel: replacement,
            state: json!({"state": "extracting"}),
        },
    );
    update(&key, &cancel, |state| state["state"] = json!("failed"));
    assert_eq!(jobs()[&key].state["state"], "extracting");
    cancel_locked(&mut jobs(), &key);
}

#[test]
fn transfer_resumes_with_range_when_server_returns_partial_content() {
    let root = tempfile::tempdir().unwrap();
    let key = (root.path().to_owned(), "272".to_owned());
    let cancel = Arc::new(AtomicBool::new(false));
    jobs().insert(
        key.clone(),
        Job {
            cancel: cancel.clone(),
            state: json!({"state": "downloading"}),
        },
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        let n = socket.read(&mut request).unwrap();
        let req_text = String::from_utf8_lossy(&request[..n]).to_ascii_lowercase();
        assert!(req_text.contains("range: bytes=1000-"), "{req_text}");
        socket
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Length: 500\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        socket.write_all(&vec![b'B'; 500]).unwrap();
    });
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut output = tempfile::tempfile().unwrap();
    output.write_all(&vec![b'A'; 1000]).unwrap();
    transfer(
        &client,
        &format!("http://{address}/pack"),
        &mut output,
        &key,
        &cancel,
    )
    .unwrap();
    server.join().unwrap();
    use std::io::{Seek, SeekFrom};
    output.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    output.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len(), 1500);
    assert!(bytes[..1000].iter().all(|&b| b == b'A'));
    assert!(bytes[1000..].iter().all(|&b| b == b'B'));
    assert_eq!(jobs()[&key].state["downloaded"], 1500);
    assert_eq!(jobs()[&key].state["total"], 1500);
    cancel_locked(&mut jobs(), &key);
}

#[test]
fn archive_and_partial_names_isolate_formats() {
    assert_eq!(archive_name("272", "mov"), "agentnave-video-272.tar.gz");
    assert_eq!(
        archive_name("272", "webm"),
        "agentnave-video-webm-272.tar.gz"
    );
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("video-packs");
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join(".272.tar.gz.part"), b"legacy-mov").unwrap();
    let mov_part = part_path(&base, "272", "mov");
    fs::write(&mov_part, b"mov-partial").unwrap();
    let webm_file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(part_path(&base, "272", "webm"))
        .unwrap();
    assert_eq!(webm_file.metadata().unwrap().len(), 0);
    assert_eq!(fs::read(&mov_part).unwrap(), b"mov-partial");
}
