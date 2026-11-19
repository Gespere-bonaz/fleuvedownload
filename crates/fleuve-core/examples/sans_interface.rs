//! Fleuve sans interface : utile pour tester le moteur et l'extension.
//!
//! cargo run -p fleuve-core --example sans_interface -- <dossier-données> <dossier-téléchargements> [jeton]

use fleuve_core::{server, Manager};
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let data = PathBuf::from(args.next().unwrap_or_else(|| "fleuve-data".into()));
    let dl = PathBuf::from(args.next().unwrap_or_else(|| "fleuve-telechargements".into()));
    let m = Manager::new(data, dl).await?;
    if let Some(token) = args.next() {
        let mut s = m.settings();
        s.extension_token = token;
        m.set_settings(s)?;
    }
    let s = m.settings();
    println!("Jeton : {}  ·  port : {}", s.extension_token, s.server_port);
    let watch = m.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            for d in watch.list() {
                println!(
                    "{:<40} {:>12?} {:>6.1}% {:>8.0} Ko/s {:?}",
                    d.file_name,
                    d.status,
                    d.progress.unwrap_or(0.0) * 100.0,
                    d.speed / 1024.0,
                    d.error
                );
            }
        }
    });
    server::serve(m.clone(), s.server_port, Arc::new(|id, _focus| println!("Reçu du navigateur : {id}"))).await
}
