use std::path::PathBuf;

/// Central location for all devstack state: ~/.devstack/
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn detect() -> crate::Result<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| crate::Error::Config("cannot determine home directory".into()))?;
        Ok(Self {
            root: home.join(".devstack"),
        })
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [
            self.root.clone(),
            self.etc_dir(),
            self.vhosts_dir(),
            self.run_dir(),
            self.logs_dir(),
            self.data_dir(),
            self.certs_dir(),
        ] {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(())
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("devstack.toml")
    }

    pub fn sites_file(&self) -> PathBuf {
        self.root.join("sites.json")
    }

    pub fn etc_dir(&self) -> PathBuf {
        self.root.join("etc")
    }

    pub fn vhosts_dir(&self) -> PathBuf {
        self.etc_dir().join("vhosts.d")
    }

    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    pub fn mysql_data_dir(&self) -> PathBuf {
        self.data_dir().join("mysql")
    }

    pub fn certs_dir(&self) -> PathBuf {
        self.root.join("certs")
    }

    /// Unix socket for a php-fpm version, e.g. run/php-fpm-8.3.sock
    pub fn php_fpm_socket(&self, version: &str) -> PathBuf {
        self.run_dir().join(format!("php-fpm-{version}.sock"))
    }

    /// pid file for a service, e.g. run/httpd.pid
    pub fn pid_file(&self, service: &str) -> PathBuf {
        self.run_dir().join(format!("{service}.pid"))
    }

    /// log file for a service, e.g. logs/httpd.log
    pub fn log_file(&self, service: &str) -> PathBuf {
        self.logs_dir().join(format!("{service}.log"))
    }

    pub fn php_ini_dir(&self, version: &str) -> PathBuf {
        self.etc_dir().join("php").join(version)
    }

    pub fn httpd_conf(&self) -> PathBuf {
        self.etc_dir().join("httpd.conf")
    }

    /// Default sites root — only used by httpd.conf generation; real value comes from Config.
    pub fn sites_root_default(&self) -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("Sites")
    }

    pub fn my_cnf(&self) -> PathBuf {
        self.etc_dir().join("my.cnf")
    }
}
