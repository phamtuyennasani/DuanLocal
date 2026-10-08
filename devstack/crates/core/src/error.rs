use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    Config(String),
    SiteExists(String),
    SiteNotFound(String),
    ServiceNotRunning(String),
    ServiceFailed { name: String, reason: String },
    PhpNotInstalled(String),
    DnsSetupRequired,
    Io(io::Error),
    Toml(toml::de::Error),
    TomlSer(toml::ser::Error),
    Json(serde_json::Error),
    Template(handlebars::RenderError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(m) => write!(f, "config: {m}"),
            Self::SiteExists(n) => write!(f, "site already exists: {n}"),
            Self::SiteNotFound(n) => write!(f, "site not found: {n}"),
            Self::ServiceNotRunning(n) => write!(f, "service not running: {n}"),
            Self::ServiceFailed { name, reason } => write!(f, "service {name} failed: {reason}"),
            Self::PhpNotInstalled(v) => write!(f, "PHP {v} not installed"),
            Self::DnsSetupRequired => write!(f, "DNS not configured — run `sudo devctl setup` first"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Toml(e) => write!(f, "toml: {e}"),
            Self::TomlSer(e) => write!(f, "toml ser: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
            Self::Template(e) => write!(f, "template: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<toml::de::Error> for Error {
    fn from(e: toml::de::Error) -> Self {
        Self::Toml(e)
    }
}
impl From<toml::ser::Error> for Error {
    fn from(e: toml::ser::Error) -> Self {
        Self::TomlSer(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl From<handlebars::RenderError> for Error {
    fn from(e: handlebars::RenderError) -> Self {
        Self::Template(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
