//! Cache command line: status, clear and the cache probe.
//!
//! These run before the window exists and before first-run Markdown association,
//! so they never open a document and never depend on GPUI.

use typors_cache::features::Features;
use typors_cache::root::CacheRoot;
use typors_cache::store::{CacheStore, EntryKind, resource_key};

/// Handle one of the cache-only invocations; None means "not a cache command".
pub fn run_if_requested() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--cache-status") {
        return Some(status());
    }
    if args.iter().any(|arg| arg == "--clear-cache") {
        return Some(clear());
    }
    if args.iter().any(|arg| arg == "--dump-cache") {
        return Some(dump());
    }
    None
}

fn store() -> (CacheRoot, Option<CacheStore>) {
    let root = CacheRoot::resolve();
    let store = if Features::global().cache {
        root.path().and_then(|path| CacheStore::open(path).ok())
    } else {
        None
    };
    (root, store)
}

fn status() -> i32 {
    let (root, store) = store();
    println!("# cache status\n");
    println!("root: {}", root.describe());
    println!("enabled: {}", store.is_some());
    match store {
        Some(mut store) => {
            store.adopt_orphans(typors_cache::store::now());
            let status = store.status();
            println!("total: {} bytes", status.total_bytes);
            println!("documents: {}", status.documents.len());
            for document in &status.documents {
                println!(
                    "  {:<20} {:>10} bytes  {}",
                    document.dir, document.bytes, document.path
                );
            }
        }
        None => {
            if Features::global().cache {
                println!("reason: cache root is not writable or could not be created");
            } else {
                println!("reason: TYPORS_CACHE is off");
            }
        }
    }
    0
}

fn clear() -> i32 {
    let (root, store) = store();
    println!("# clear cache\n");
    println!("root: {}", root.describe());
    match store {
        Some(mut store) => match store.clear() {
            Ok(()) => {
                println!("cleared");
                0
            }
            Err(error) => {
                eprintln!("typors: cannot clear cache: {error}");
                1
            }
        },
        None => {
            println!("nothing to clear");
            0
        }
    }
}

fn dump() -> i32 {
    let resolved = CacheRoot::resolve();
    let temp = std::env::temp_dir().join(format!("typors-cache-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let mut checks: Vec<(String, bool, String)> = Vec::new();

    let mut store = match CacheStore::open(&temp) {
        Ok(store) => store,
        Err(error) => {
            println!("# cache dump\n\nroot: {}", resolved.describe());
            println!("  [FAIL] probe cache could not be created: {error}");
            println!("\nresult: FAIL");
            return 1;
        }
    };

    let path = r"C:\probe\document.md";
    let first = store.register(path, 100).map(|cache| cache.dir_name().to_string());
    let second = store.register(path, 200).map(|cache| cache.dir_name().to_string());
    checks.push((
        "C1 mapping is stable across registrations".to_string(),
        first.is_ok() && first == second,
        format!("{first:?} / {second:?}"),
    ));

    let cache = store.register(path, 300).unwrap();
    let key = resource_key(b"probe-payload");
    let written = cache.write(EntryKind::Image, &key, b"0123456789");
    let read = cache.read(EntryKind::Image, &key);
    checks.push((
        "C2 entry round-trip".to_string(),
        written.is_ok() && read.as_deref() == Some(&b"0123456789"[..]),
        format!("write={written:?} read={read:?}"),
    ));

    let entry = cache.entry_path(EntryKind::Image, &key);
    let mut bytes = std::fs::read(&entry).unwrap_or_default();
    if let Some(last) = bytes.last_mut() {
        *last ^= 0xff;
    }
    let mutated_read = std::fs::write(&entry, &bytes)
        .ok()
        .and_then(|_| cache.read(EntryKind::Image, &key));
    checks.push((
        "C3 checksum rejects a mutated byte".to_string(),
        mutated_read.is_none(),
        format!("{:?}", mutated_read.map(|payload| payload.len())),
    ));

    let truncated = bytes.get(..bytes.len().saturating_sub(3)).map(|slice| slice.to_vec());
    let truncated_read = truncated
        .and_then(|slice| std::fs::write(&entry, slice).ok())
        .and_then(|_| cache.read(EntryKind::Image, &key));
    checks.push((
        "C4 truncated entry is a miss".to_string(),
        truncated_read.is_none(),
        format!("{:?}", truncated_read.map(|payload| payload.len())),
    ));

    let other = store.register(r"C:\probe\other.md", 50).unwrap();
    other.write(EntryKind::Raster, "k", &[9u8; 2048]).unwrap();
    cache.write(EntryKind::Raster, "k", &[8u8; 2048]).unwrap();
    store.close_document(other.path());
    let before = store.status().total_bytes;
    let gc = store.gc(before.saturating_sub(1));
    checks.push((
        "C5 LRU eviction removes the closed document".to_string(),
        gc.removed_documents == 1 && !other.dir().exists(),
        format!("{:?}", gc),
    ));

    let cleared = store.clear();
    let emptied = store.status().documents.is_empty() && !cache.dir().exists();
    checks.push((
        "C6 clear removes the tree".to_string(),
        cleared.is_ok() && emptied,
        format!("clear={cleared:?} emptied={emptied}"),
    ));

    let _ = std::fs::remove_dir_all(&temp);
    let ok = checks.iter().all(|(_, pass, _)| *pass);
    println!("# cache dump\n");
    println!("root: {}", resolved.describe());
    println!("features: cache={}\n", Features::global().cache);
    for (name, pass, detail) in &checks {
        println!("  [{}] {name} — {detail}", if *pass { "ok" } else { "FAIL" });
    }
    println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
