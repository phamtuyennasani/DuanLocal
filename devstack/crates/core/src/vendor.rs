use std::path::PathBuf;

/// Vendor directory: ~/.devstack/vendor/ holds downloaded binaries on Windows.
/// On macOS this is unused — binaries come from brew.
pub fn vendor_dir(paths: &crate::paths::Paths) -> PathBuf {
    paths.root.join("vendor")
}

/// Find a binary inside a vendored package directory.
/// E.g. find_vendor_bin("php-8.3", "php-cgi.exe") →
///   ~/.devstack/vendor/php-8.3/php-cgi.exe
pub fn find_vendor_bin(package: &str, bin_rel: &str) -> Option<PathBuf> {
    let root = crate::paths::Paths::detect().ok()?.root.join("vendor").join(package);
    let p = root.join(bin_rel);
    p.exists().then_some(p)
}

/// List installed vendor packages (directories under vendor/).
pub fn list_vendored() -> Vec<String> {
    let Ok(paths) = crate::paths::Paths::detect() else { return vec![] };
    let dir = vendor_dir(&paths);
    if !dir.exists() {
        return vec![];
    }
    std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A resolved package: where to fetch it and what to expect.
#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    /// Logical name, e.g. "php-8.3" — also the vendor dir name.
    pub package: String,
    /// Direct download URL.
    pub url: String,
    /// Expected filename (for display / extension check).
    pub filename: String,
    /// sha256 hex digest if known (PHP releases.json supplies it).
    pub sha256: Option<String>,
    /// Human-readable size if known.
    pub size_hint: Option<String>,
}

/// Progress callback phases for install.
#[derive(Debug, Clone)]
pub enum Progress {
    /// Resolving URL / metadata.
    Resolving,
    /// Downloading: bytes fetched so far, total if known.
    Downloading { got: u64, total: Option<u64> },
    /// Extracting files.
    Extracting,
    /// Done — path installed to.
    Done(PathBuf),
}

/// Resolve a package spec ("php-8.3", "mariadb", "httpd") to a download.
/// Network: hits the PHP releases.json / ApacheLounge listing as needed.
pub fn resolve(package: &str) -> crate::Result<ResolvedPackage> {
    if let Some(ver) = package.strip_prefix("php-") {
        resolve_php(package, ver)
    } else {
        match package {
            "mariadb" => resolve_mariadb(),
            "httpd" => resolve_httpd(),
            _ => Err(crate::Error::Config(format!(
                "unknown package: {package} — expected php-X.Y | mariadb | httpd"
            ))),
        }
    }
}

/// List PHP minor versions available upstream (from releases.json keys).
pub fn available_php_versions() -> crate::Result<Vec<String>> {
    let json = fetch_php_releases()?;
    let mut out: Vec<String> = json
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    // releases.json keys are like "8.3" — keep only 8.x+
    out.retain(|v| v.starts_with("8."));
    out.sort();
    Ok(out)
}

fn fetch_php_releases() -> crate::Result<serde_json::Value> {
    let url = "https://downloads.php.net/~windows/releases/releases.json";
    let body = reqwest::blocking::get(url)
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| crate::Error::Config(format!("fetch php releases: {e}")))?;
    serde_json::from_str(&body)
        .map_err(|e| crate::Error::Config(format!("parse php releases.json: {e}")))
}

fn resolve_php(package: &str, ver: &str) -> crate::Result<ResolvedPackage> {
    let json = fetch_php_releases()?;
    let branch = json.get(ver).ok_or_else(|| {
        crate::Error::Config(format!("php-{ver} not found upstream (supported: {})",
            available_php_versions().unwrap_or_default().join(", ")))
    })?;

    // pick the best NTS x64 build: prefer vs17 (8.4+), then vs16, then vc15
    let variant = ["nts-vs17-x64", "nts-vs16-x64", "nts-vc15-x64"]
        .iter()
        .find(|k| branch.get(**k).is_some())
        .ok_or_else(|| crate::Error::Config(format!("no NTS x64 build for php-{ver}")))?;

    let zip = &branch[*variant]["zip"];
    let filename = zip["path"].as_str().unwrap_or("").to_string();
    if filename.is_empty() {
        return Err(crate::Error::Config(format!("no zip for php-{ver} {variant}")));
    }
    let sha256 = zip["sha256"].as_str().map(String::from);
    let size_hint = zip["size"].as_str().map(String::from);

    Ok(ResolvedPackage {
        package: package.to_string(),
        url: format!("https://downloads.php.net/~windows/releases/{filename}"),
        filename,
        sha256,
        size_hint,
    })
}

fn resolve_mariadb() -> crate::Result<ResolvedPackage> {
    // Latest 11.4.x LTS — archive.mariadb.org lists winx64 zips per version dir.
    // Pinned to a known-good release; bump to update.
    let version = "11.4.2";
    let filename = format!("mariadb-{version}-winx64.zip");
    Ok(ResolvedPackage {
        package: "mariadb".to_string(),
        url: format!("https://archive.mariadb.org/mariadb-{version}/winx64-packages/{filename}"),
        filename,
        sha256: None,
        size_hint: None,
    })
}

fn resolve_httpd() -> crate::Result<ResolvedPackage> {
    // ApacheLounge serves direct zips under /download/VSXX/binaries/.
    // Their zips contain a top-level "Apache24/" dir which our extractor strips.
    // Pinned to the latest VS18 build found on the download page.
    let filename = "httpd-2.4.69-261002-Win64-VS18.zip";
    Ok(ResolvedPackage {
        package: "httpd".to_string(),
        url: format!("https://www.apachelounge.com/download/VS18/binaries/{filename}"),
        filename: filename.to_string(),
        sha256: None,
        size_hint: Some("~14 MB".to_string()),
    })
}

/// Download + extract a package into the vendor dir.
/// `on_progress` is called with Downloading updates (throttled by caller cadence).
///
/// Extraction strips the archive's top-level directory:
///   php-8.3.35-nts-Win32-vs16-x64/php-cgi.exe → php-cgi.exe
///   Apache24/bin/httpd.exe → bin/httpd.exe
///   mariadb-11.4.2-winx64/bin/mariadbd.exe → bin/mariadbd.exe
/// PHP zips are flat (no top dir) so files land directly.
pub fn install(
    paths: &crate::paths::Paths,
    package: &str,
    on_progress: impl Fn(Progress),
) -> crate::Result<PathBuf> {
    on_progress(Progress::Resolving);
    let pkg = resolve(package)?;

    let dest_dir = vendor_dir(paths).join(&pkg.package);
    if dest_dir.exists() {
        return Ok(dest_dir);
    }

    // download to tmp dir outside vendor so partial files never look installed
    let tmp_dir = paths.root.join("tmp");
    std::fs::create_dir_all(&tmp_dir)?;
    let zip_path = tmp_dir.join(format!("{}.zip", pkg.package));

    download(&pkg, &zip_path, &on_progress)?;

    if let Some(want) = &pkg.sha256 {
        verify_sha256(&zip_path, want)?;
    }

    on_progress(Progress::Extracting);
    extract_zip(&zip_path, &dest_dir)?;
    let _ = std::fs::remove_file(&zip_path);

    on_progress(Progress::Done(dest_dir.clone()));
    Ok(dest_dir)
}

fn download(
    pkg: &ResolvedPackage,
    zip_path: &PathBuf,
    on_progress: &impl Fn(Progress),
) -> crate::Result<()> {
    let mut resp = reqwest::blocking::get(&pkg.url)
        .map_err(|e| crate::Error::Config(format!("download {}: {e}", pkg.url)))?;
    if !resp.status().is_success() {
        return Err(crate::Error::Config(format!(
            "HTTP {} downloading {}",
            resp.status(),
            pkg.url
        )));
    }
    let total = resp.content_length();
    let mut out = std::fs::File::create(zip_path)?;
    let mut got: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    loop {
        use std::io::Read;
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        use std::io::Write;
        out.write_all(&buf[..n])?;
        got += n as u64;
        on_progress(Progress::Downloading { got, total });
    }
    Ok(())
}

fn verify_sha256(path: &PathBuf, want: &str) -> crate::Result<()> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut ctx = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    let got = ctx.hex();
    if !got.eq_ignore_ascii_case(want) {
        return Err(crate::Error::Config(format!(
            "sha256 mismatch — got {got}, expected {want}"
        )));
    }
    Ok(())
}

/// Extract a zip, stripping the first path component of each entry.
/// Flat zips (PHP) pass through untouched since they have no top dir.
fn extract_zip(zip_path: &PathBuf, dest_dir: &PathBuf) -> crate::Result<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| crate::Error::Config(format!("not a zip ({}): {e}", zip_path.display())))?;

    // Decide how many leading components to strip. Some zips nest everything
    // under a single dir (Apache24/, mariadb-x.y.z-winx64/) but also scatter a
    // few loose files at the root (ReadMe.txt, "-- Win64 VS18 --"). Rather than
    // require a common prefix, strip the single top-level directory that
    // actually contains the package payload. For PHP (flat zips) there's no
    // such dir → strip 0.
    let strip = detect_strip_root(&mut archive);

    // extract to a staging dir, then rename into place — never leave a
    // half-extracted package looking installed
    let staging = dest_dir.with_extension("staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;

    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| {
            crate::Error::Config(format!("zip entry {i}: {e}"))
        })?;
        let name = match f.enclosed_name() {
            Some(n) => n.to_string_lossy().replace('\\', "/"),
            None => continue, // zip-slip guard
        };
        let rel: String = if let Some(root) = &strip {
            // only descend into entries under the payload root; drop loose
            // root-level files (ReadMe.txt, "-- Win64 VS18 --") entirely
            match name.strip_prefix(&format!("{root}/")) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => continue,
            }
        } else {
            name.clone()
        };
        if rel.ends_with('/') || rel.is_empty() {
            continue;
        }
        let out_path = staging.join(&rel);
        std::fs::create_dir_all(out_path.parent().unwrap())?;
        let mut outfile = std::fs::File::create(&out_path)?;
        std::io::copy(&mut f, &mut outfile)?;
    }

    if dest_dir.exists() {
        std::fs::remove_dir_all(dest_dir)?;
    }
    std::fs::rename(&staging, dest_dir)?;
    Ok(())
}

/// Find the single top-level directory that holds the package payload.
/// Returns its name (e.g. "Apache24", "mariadb-11.4.2-winx64") to strip,
/// or None for flat zips (PHP).
///
/// Heuristic: look for the dir that contains a known binary marker. Falls back
/// to "the dir that owns the most entries" when markers aren't present.
fn detect_strip_root(archive: &mut zip::ZipArchive<std::fs::File>) -> Option<String> {
    use std::collections::HashMap;

    // markers that identify the payload dir per package
    const MARKERS: &[&str] = &["bin/httpd.exe", "bin/mariadbd.exe", "bin/mysqld.exe"];

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut marker_root: Option<String> = None;

    for i in 0..archive.len() {
        let Ok(f) = archive.by_index(i) else { continue };
        let Some(name) = f.enclosed_name() else { continue };
        let norm = name.to_string_lossy().replace('\\', "/");
        let mut it = norm.splitn(2, '/');
        let first = it.next().unwrap_or("");
        if let Some(rest) = it.next() {
            // entry lives under a top dir
            *counts.entry(first.to_string()).or_default() += 1;
            if MARKERS.iter().any(|m| rest.eq_ignore_ascii_case(m)) {
                marker_root = Some(first.to_string());
            }
        }
    }

    if marker_root.is_some() {
        return marker_root;
    }
    // fallback: if exactly one top dir owns >80% of entries, strip it
    let total: usize = counts.values().sum();
    if total == 0 {
        return None; // flat zip (PHP)
    }
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    let (top, count) = sorted[0];
    if *count as f64 / total as f64 > 0.8 {
        Some(top.clone())
    } else {
        None
    }
}

/// Minimal SHA-256 (FIPS 180-4) — avoids pulling a crypto dep for one hash.
struct Sha256 {
    h: [u32; 8],
    len: u64,
    buf: Vec<u8>,
}
impl Sha256 {
    fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
            ],
            len: 0,
            buf: Vec::new(),
        }
    }
    fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if !self.buf.is_empty() {
            let need = 64 - self.buf.len();
            let take = need.min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == 64 {
                let block = std::mem::take(&mut self.buf);
                self.compress(&block);
            }
        }
        while data.len() >= 64 {
            self.compress(&data[..64]);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf.extend_from_slice(data);
        }
    }
    fn hex(mut self) -> String {
        let bitlen = self.len * 8;
        self.update(&[0x80]);
        while self.buf.len() % 64 != 56 {
            self.update(&[0]);
        }
        self.update(&bitlen.to_be_bytes());
        let mut out = String::new();
        for w in self.h {
            out.push_str(&format!("{w:08x}"));
        }
        out
    }
    fn compress(&mut self, block: &[u8]) {
        const K: [u32; 64] = [
            0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
            0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
            0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
            0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
            0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
            0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
            0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
            0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
        ];
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[i*4], block[i*4+1], block[i*4+2], block[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let [mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut h] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6)^e.rotate_right(11)^e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2)^a.rotate_right(13)^a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h=g; g=f; f=e; e=d.wrapping_add(t1); d=c; c=b; b=a; a=t1.wrapping_add(t2);
        }
        self.h[0]=self.h[0].wrapping_add(a); self.h[1]=self.h[1].wrapping_add(b);
        self.h[2]=self.h[2].wrapping_add(c); self.h[3]=self.h[3].wrapping_add(d);
        self.h[4]=self.h[4].wrapping_add(e); self.h[5]=self.h[5].wrapping_add(f);
        self.h[6]=self.h[6].wrapping_add(g); self.h[7]=self.h[7].wrapping_add(h);
    }
}
