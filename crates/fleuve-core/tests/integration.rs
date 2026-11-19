//! Tests de bout en bout contre un serveur HTTP local qui imite les cas réels :
//! plages d'octets, serveur sans plages, HLS, flux chiffré, erreur 404.

use axum::body::Body;
use axum::extract::{Path as AxPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use fleuve_core::{AddRequest, Kind, Manager, Settings, Status};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const SIZE: usize = 24 * 1024 * 1024 + 12345;

#[derive(Clone)]
struct Srv {
    data: Arc<Vec<u8>>,
}

fn make_data(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x & 0xff) as u8
        })
        .collect()
}

fn parse_range(h: &HeaderMap, len: usize) -> Option<(usize, usize)> {
    let v = h.get(header::RANGE)?.to_str().ok()?.strip_prefix("bytes=")?;
    let (a, b) = v.split_once('-')?;
    let a: usize = a.parse().ok()?;
    let b: usize = if b.is_empty() { len - 1 } else { b.parse().ok()? };
    Some((a, b.min(len - 1)))
}

/// Envoie `bytes` par morceaux de 64 Ko avec une pause entre chaque.
fn slow_body(bytes: Vec<u8>, delay_ms: u64) -> Body {
    let stream = futures_util::stream::unfold((bytes, 0usize), move |(b, i)| async move {
        if i >= b.len() {
            return None;
        }
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        let end = (i + 65536).min(b.len());
        let chunk = axum::body::Bytes::copy_from_slice(&b[i..end]);
        Some((Ok::<_, std::io::Error>(chunk), (b, end)))
    });
    Body::from_stream(stream)
}

fn ranged(data: &[u8], headers: &HeaderMap, delay_ms: u64) -> Response {
    let len = data.len();
    match parse_range(headers, len) {
        Some((a, b)) if a < len => Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {a}-{b}/{len}"))
            .header(header::CONTENT_LENGTH, b - a + 1)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::ETAG, "\"v1\"")
            .body(slow_body(data[a..=b].to_vec(), delay_ms))
            .unwrap(),
        Some(_) => StatusCode::RANGE_NOT_SATISFIABLE.into_response(),
        None => Response::builder()
            .header(header::CONTENT_LENGTH, len)
            .body(slow_body(data.to_vec(), delay_ms))
            .unwrap(),
    }
}

async fn file(State(s): State<Srv>, h: HeaderMap) -> Response {
    ranged(&s.data, &h, 1)
}
async fn slow(State(s): State<Srv>, h: HeaderMap) -> Response {
    ranged(&s.data, &h, 60)
}
async fn named(State(s): State<Srv>, h: HeaderMap) -> Response {
    let mut r = ranged(&s.data[..3 * 1024 * 1024], &h, 0);
    r.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        "attachment; filename*=UTF-8''rapport%20%C3%A9t%C3%A9.pdf".parse().unwrap(),
    );
    r
}
async fn norange(State(s): State<Srv>) -> Response {
    Response::builder()
        .header(header::CONTENT_LENGTH, 3 * 1024 * 1024)
        .body(slow_body(s.data[..3 * 1024 * 1024].to_vec(), 0))
        .unwrap()
}

fn seg_bytes(variant: &str, i: usize) -> Vec<u8> {
    make_data(200_000 + i * 1000, if variant == "hi" { 77 + i as u32 } else { 5 })
}
async fn hls(AxPath(p): AxPath<String>) -> Response {
    let text = |t: String| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
            .body(Body::from(t))
            .unwrap()
    };
    match p.as_str() {
        "master.m3u8" => text("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=500000\nlo/v.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=3000000,CODECS=\"avc1,mp4a\"\nhi/v.m3u8\n".into()),
        "lo/v.m3u8" | "hi/v.m3u8" => {
            let mut s = String::from("#EXTM3U\n#EXT-X-TARGETDURATION:4\n");
            for i in 0..12 {
                s.push_str(&format!("#EXTINF:4.0,\nseg{i}.ts\n"));
            }
            s.push_str("#EXT-X-ENDLIST\n");
            text(s)
        }
        "enc.m3u8" => text("#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF:4,\nhi/seg0.ts\n#EXT-X-ENDLIST\n".into()),
        other => {
            let (variant, rest) = other.split_once('/').unwrap_or(("", other));
            match rest.strip_prefix("seg").and_then(|r| r.strip_suffix(".ts")).and_then(|n| n.parse().ok()) {
                Some(i) => Body::from(seg_bytes(variant, i)).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
    }
}

async fn start_server() -> (String, Arc<Vec<u8>>) {
    let data = Arc::new(make_data(SIZE, 2463534242));
    let app = Router::new()
        .route("/file.bin", get(file))
        .route("/slow.bin", get(slow))
        .route("/named", get(named))
        .route("/norange.bin", get(norange))
        .route("/hls/{*p}", get(hls))
        .with_state(Srv { data: data.clone() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), data)
}

async fn manager(dir: &Path) -> Manager {
    let m = Manager::new(dir.join("data"), dir.join("dl")).await.unwrap();
    let mut s: Settings = m.settings();
    s.sort_by_category = false;
    s.connections = 8;
    m.set_settings(s).unwrap();
    m
}

async fn wait_for(m: &Manager, id: &str, secs: u64, pred: impl Fn(Status) -> bool) -> Status {
    for _ in 0..secs * 20 {
        let st = m.get(id).unwrap().status;
        if pred(st) {
            return st;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("délai dépassé, statut = {:?} / {:?}", m.get(id).unwrap().status, m.get(id).unwrap().error);
}

fn add(m: &Manager, url: String) -> String {
    m.add(AddRequest { url, ..Default::default() }).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn segmented_download_is_exact() {
    let (base, data) = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path()).await;
    let id = add(&m, format!("{base}/file.bin"));
    let st = wait_for(&m, &id, 30, |s| matches!(s, Status::Completed | Status::Failed)).await;
    let v = m.get(&id).unwrap();
    assert_eq!(st, Status::Completed, "{:?}", v.error);
    assert!(v.segments.len() >= 4, "segmentation dynamique attendue, {} segments", v.segments.len());
    let got = std::fs::read(tmp.path().join("dl/file.bin")).unwrap();
    assert_eq!(got.len(), data.len());
    assert!(got == *data, "contenu différent");
    assert!(!tmp.path().join("dl/file.bin.fleuvepart").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_resume_and_restart() {
    let (base, data) = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let id;
    {
        let m = manager(tmp.path()).await;
        id = add(&m, format!("{base}/slow.bin"));
        wait_for(&m, &id, 10, |s| s == Status::Downloading).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        m.pause(&id).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let v = m.get(&id).unwrap();
        assert_eq!(v.status, Status::Paused);
        assert!(v.received > 0 && v.received < data.len() as u64, "reçu {}", v.received);
        // Reprise dans la même session puis nouvelle pause.
        m.resume(&id).unwrap();
        wait_for(&m, &id, 10, |s| s == Status::Downloading).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        m.pause(&id).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    // « Redémarrage » de l'application : nouvel objet, même dossier de données.
    let m2 = Manager::new(tmp.path().join("data"), tmp.path().join("dl")).await.unwrap();
    let before = m2.get(&id).unwrap();
    assert_eq!(before.status, Status::Paused);
    assert!(before.received > 0);
    m2.resume(&id).unwrap();
    let st = wait_for(&m2, &id, 60, |s| matches!(s, Status::Completed | Status::Failed)).await;
    assert_eq!(st, Status::Completed, "{:?}", m2.get(&id).unwrap().error);
    let got = std::fs::read(tmp.path().join("dl/slow.bin")).unwrap();
    assert!(got == *data, "contenu différent après reprise");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_without_ranges_and_disposition_name() {
    let (base, data) = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path()).await;
    let a = add(&m, format!("{base}/norange.bin"));
    let b = add(&m, format!("{base}/named"));
    assert_eq!(wait_for(&m, &a, 20, |s| matches!(s, Status::Completed | Status::Failed)).await, Status::Completed);
    assert_eq!(wait_for(&m, &b, 20, |s| matches!(s, Status::Completed | Status::Failed)).await, Status::Completed);
    let va = m.get(&a).unwrap();
    assert!(!va.resumable);
    assert_eq!(std::fs::read(tmp.path().join("dl/norange.bin")).unwrap(), data[..3 * 1024 * 1024]);
    let vb = m.get(&b).unwrap();
    assert_eq!(vb.file_name, "rapport été.pdf");
    assert_eq!(vb.category, "Documents");
    assert!(tmp.path().join("dl/rapport été.pdf").exists());
    // Même lien une 2e fois : nom libre « (1) ».
    let c = add(&m, format!("{base}/named"));
    wait_for(&m, &c, 20, |s| s == Status::Completed).await;
    assert_eq!(m.get(&c).unwrap().file_name, "rapport été (1).pdf");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hls_best_variant_concatenated() {
    let (base, _) = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path()).await;
    let id = add(&m, format!("{base}/hls/master.m3u8"));
    assert_eq!(m.get(&id).unwrap().kind, Kind::Hls);
    let st = wait_for(&m, &id, 30, |s| matches!(s, Status::Completed | Status::Failed)).await;
    let v = m.get(&id).unwrap();
    assert_eq!(st, Status::Completed, "{:?}", v.error);
    assert_eq!(v.file_name, "master.ts");
    assert_eq!(v.hls_total_parts, 12);
    let expected: Vec<u8> = (0..12).flat_map(|i| seg_bytes("hi", i)).collect();
    let got = std::fs::read(tmp.path().join("dl/master.ts")).unwrap();
    assert!(got == expected, "concaténation incorrecte");
    assert!(!tmp.path().join("dl/.master.ts.fleuve-hls").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refuses_encrypted_hls_and_reports_404() {
    let (base, _) = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path()).await;
    let enc = add(&m, format!("{base}/hls/enc.m3u8"));
    let missing = add(&m, format!("{base}/absent.zip"));
    assert_eq!(wait_for(&m, &enc, 10, |s| matches!(s, Status::Completed | Status::Failed)).await, Status::Failed);
    assert!(m.get(&enc).unwrap().error.unwrap().contains("chiffré"));
    assert_eq!(wait_for(&m, &missing, 10, |s| matches!(s, Status::Completed | Status::Failed)).await, Status::Failed);
    assert!(m.get(&missing).unwrap().error.unwrap().contains("404"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_respects_max_concurrent_and_schedule() {
    let (base, _) = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path()).await;
    let mut s = m.settings();
    s.max_concurrent = 1;
    m.set_settings(s).unwrap();
    let a = add(&m, format!("{base}/slow.bin"));
    let b = add(&m, format!("{base}/file.bin"));
    let later = m
        .add(AddRequest {
            url: format!("{base}/norange.bin"),
            scheduled_at: Some(fleuve_core::util::now_secs() + 3600),
            ..Default::default()
        })
        .unwrap();
    wait_for(&m, &a, 10, |s| s == Status::Downloading).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(m.get(&b).unwrap().status, Status::Queued);
    assert_eq!(m.get(&later).unwrap().status, Status::Scheduled);
    m.pause(&a).unwrap();
    wait_for(&m, &b, 10, |s| s != Status::Queued).await;
    // Programmé dans le passé : démarre au prochain passage du planificateur.
    m.schedule(&later, fleuve_core::util::now_secs() - 1).unwrap();
    m.remove(&a, true).unwrap();
    wait_for(&m, &later, 30, |s| s == Status::Completed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speed_limit_is_applied() {
    let (base, _) = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path()).await;
    let mut s = m.settings();
    s.speed_limit_kbps = 1024; // 1 Mo/s
    m.set_settings(s).unwrap();
    let t0 = std::time::Instant::now();
    let id = add(&m, format!("{base}/norange.bin")); // 3 Mo
    wait_for(&m, &id, 20, |s| s == Status::Completed).await;
    let secs = t0.elapsed().as_secs_f64();
    assert!(secs > 1.6, "trop rapide pour 1 Mo/s : {secs:.2} s");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extension_api_requires_token_and_extension_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path()).await;
    let token = m.settings().extension_token;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = fleuve_core::server::router(m.clone(), Arc::new(|_, _| {}));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let c = reqwest::Client::new();
    let url = format!("http://{addr}/add");
    let body = serde_json::json!({"url": "https://example.org/a.zip"});

    let r = c.post(&url).header("content-type", "application/json").body(body.to_string()).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = c.post(&url).header("x-fleuve-token", &token).header("origin", "https://site-malveillant.example")
        .header("content-type", "application/json").body(body.to_string()).send().await.unwrap();
    assert_eq!(r.status(), 403);
    let r = c.post(&url).header("x-fleuve-token", &token).header("origin", "chrome-extension://abcdef")
        .header("content-type", "application/json").body(body.to_string()).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let r = c.get(format!("http://{addr}/ping")).header("x-fleuve-token", &token).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(m.list().len(), 1);
}
