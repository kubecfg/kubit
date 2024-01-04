use base64::{engine::general_purpose, Engine as _};
use oci_distribution::secrets::RegistryAuth;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Error deserializing JSON: {0}")]
    Deserializing(#[from] serde_json::Error),

    #[error("Error decoding base64 field inside docker config auth section: {0}")]
    Base64Decode(#[from] base64::DecodeError),

    #[error("Error decoding content of base64 field inside docker config auth section: {0}")]
    FromUtf8(#[from] std::string::FromUtf8Error),

    #[error("Missing colon in auth field")]
    MissingColon,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A content of ~/.docker/config.json file which is also the same format
/// of the contents of the kubernetes kubernetes.io/dockerconfigjson secret
#[derive(Clone, Deserialize)]
pub struct DockerConfig {
    auths: HashMap<String, DockerCredentials>,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub enum DockerCredentials {
    Split { username: String, password: String },
    Composite { auth: String },
}

impl DockerConfig {
    /// Parse a DockerConfig from a string.
    #[allow(dead_code)]
    pub fn from_str(str: &str) -> Result<Self> {
        Self::from_slice(str.as_bytes())
    }

    /// Parse a DockerConfig from slice of bytes.
    pub fn from_slice(data: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(data)?)
    }

    /// Returns a [`RegistryAuth`] for a given image registry.
    /// If a registry is not mentioned in the auth section of the docker config file,
    /// the authentication method will be "anonymous" (i.e. unauthenticated), which
    /// is suitable for public images. This matches the normal behavior of the docker client.
    pub fn get_auth(&self, registry: &str) -> Result<RegistryAuth> {
        Ok(match self.auths.get(registry) {
            None => RegistryAuth::Anonymous,
            Some(credentials) => {
                let (username, password) = credentials.unpack()?;
                RegistryAuth::Basic(username, password)
            }
        })
    }
}

impl DockerCredentials {
    fn unpack(&self) -> Result<(String, String)> {
        Ok(match self.clone() {
            DockerCredentials::Split { username, password } => (username, password),

            DockerCredentials::Composite { auth } => {
                String::from_utf8(general_purpose::STANDARD.decode(auth)?)?
                    .split_once(':')
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .ok_or(Error::MissingColon)?
            }
        })
    }
}

impl std::fmt::Debug for DockerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerConfig")
            .field("auths", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn with_username_password() {
        let src = r#"
        {
            "auths": {
                "us-docker.pkg.dev": {
                    "username": "foo",
                    "password": "hunter12"
                }
            }
        }
        "#;

        let config = DockerConfig::from_str(src).expect("no errors");

        let auth = config.get_auth("us-docker.pkg.dev").expect("no errors");
        assert_matches!(auth, RegistryAuth::Basic(username, password) if username == "foo" && password == "hunter12");

        let auth = config.get_auth("registry.k8s.io").expect("no errors");
        assert_matches!(auth, RegistryAuth::Anonymous);
    }

    #[test]
    fn with_auth() {
        let src = r#"
        {
            "auths": {
                "us-docker.pkg.dev": {
                    "auth": "Zm9vOmh1bnRlcjEy"
                }
            }
        }
        "#;

        let config = DockerConfig::from_str(src).expect("no errors");
        let auth = config.get_auth("us-docker.pkg.dev").expect("no errors");
        assert_matches!(auth, RegistryAuth::Basic(username, password) if username == "foo" && password == "hunter12");

        let auth = config.get_auth("bitnami/kubectl").expect("no errors");
        assert_matches!(auth, RegistryAuth::Anonymous);
    }

    #[test]
    fn with_auth_padding() {
        let src = r#"
        {
            "auths": {
                "us-docker.pkg.dev": {
                    "auth": "IWw+aFk6LWtHUS1xWg=="
                }
            }
        }
        "#;

        let config = DockerConfig::from_str(src).expect("no errors");
        let auth = config.get_auth("us-docker.pkg.dev").expect("no errors");
        assert_matches!(auth, RegistryAuth::Basic(username, password) if username == "!l>hY" && password == "-kGQ-qZ");

        let auth = config.get_auth("bitnami/kubectl").expect("no errors");
        assert_matches!(auth, RegistryAuth::Anonymous);
    }

    #[test]
    fn other_fields() {
        let src = r#"
        {
            "auths": {
                "us-docker.pkg.dev": {
                    "auth": "Zm9vOmh1bnRlcjEy"
                }
            },
            "credsStore": "desktop"
        }
        "#;

        DockerConfig::from_str(src).expect("no errors");
    }

    #[test]
    fn bad_json() {
        let src = r#"
        {
            "auths": {
                "us-docker.pkg.dev": {
                    "auth": "Zm9vOmh1bnRlcjEy"
                },
            }
        }
        "#;

        assert_matches!(DockerConfig::from_str(src), Err(Error::Deserializing(_)));
    }

    #[test]
    fn bad_auth() {
        let src = r#"
        {
            "auths": {
                "us-docker.pkg.dev": {
                    "auth": "Zm9v"
                }
            }
        }
        "#;

        let config = DockerConfig::from_str(src).expect("no errors");
        assert_matches!(
            config.get_auth("us-docker.pkg.dev"),
            Err(Error::MissingColon)
        );
    }
}
