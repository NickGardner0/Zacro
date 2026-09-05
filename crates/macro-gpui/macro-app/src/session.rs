use macro_api::UserInfo;
use serde::{Deserialize, Serialize};
use std::path::Path;

const DEVELOPMENT_CREDENTIALS_KEY: &str = "zacro://session";
const PACKAGED_CREDENTIALS_KEY: &str = "zacro-app://session";

pub fn credentials_key() -> &'static str {
    credentials_key_for_executable(std::env::current_exe().ok().as_deref())
}

pub fn legacy_credentials_key() -> &'static str {
    if credentials_key() == PACKAGED_CREDENTIALS_KEY {
        DEVELOPMENT_CREDENTIALS_KEY
    } else {
        PACKAGED_CREDENTIALS_KEY
    }
}

fn credentials_key_for_executable(executable: Option<&Path>) -> &'static str {
    if executable
        .and_then(Path::to_str)
        .is_some_and(|path| path.contains(".app/Contents/MacOS/"))
    {
        PACKAGED_CREDENTIALS_KEY
    } else {
        DEVELOPMENT_CREDENTIALS_KEY
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub environment: String,
    pub access_token: String,
    pub refresh_token: String,
    pub macro_api_token: String,
}

#[derive(Debug, Clone)]
pub enum SessionState {
    Restoring,
    SignedOut,
    SendingCode,
    AwaitingCode { email: String },
    Verifying { email: String },
    SsoRequired { email: String, url: String },
    Ready { user: UserInfo },
    Failed { message: String },
}

impl SessionState {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Restoring | Self::SendingCode | Self::Verifying { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthField {
    Email,
    Code,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_and_development_executables_use_stable_distinct_namespaces() {
        assert_eq!(
            credentials_key_for_executable(Some(Path::new(
                "/Applications/Zacro.app/Contents/MacOS/Zacro"
            ))),
            PACKAGED_CREDENTIALS_KEY
        );
        assert_eq!(
            credentials_key_for_executable(Some(Path::new("/tmp/target/debug/macro_app"))),
            DEVELOPMENT_CREDENTIALS_KEY
        );
        assert_ne!(credentials_key(), legacy_credentials_key());
    }
}
