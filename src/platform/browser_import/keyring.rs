//! Fetches the key material a Chromium browser keeps in the OS credential store.

use super::{Chromium, Failure, chromium::Keys};

/// The login keychain holds one password per browser. `security` asks the
/// user for access itself when the keychain requires it.
#[cfg(target_os = "macos")]
pub(super) fn keys(browser: &Chromium) -> Result<Keys, Failure> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-w", "-s"])
        .arg(format!("{} Safe Storage", browser.keychain))
        .args(["-a", browser.keychain])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|_| Failure::KeyringLocked)?;
    if !output.status.success() {
        // 44 is errSecItemNotFound; every other refusal is one of access.
        let missing = output.status.code() == Some(44)
            || String::from_utf8_lossy(&output.stderr).contains("could not be found");
        return Err(if missing {
            Failure::KeyringMissing
        } else {
            Failure::KeyringLocked
        });
    }
    let secret = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
    Ok(Keys {
        v10: super::chromium::derive(secret, 1003),
        v11: None,
        v11_failure: None,
        empty: None,
    })
}

/// `v10` values use a fixed password; only `v11` ones need the keyring, so
/// its failure is carried along rather than returned.
#[cfg(target_os = "linux")]
pub(super) fn keys(browser: &Chromium) -> Result<Keys, Failure> {
    use super::chromium::derive;
    let secret = linux::session().and_then(|bus| linux::secret(&bus, browser.application));
    Ok(Keys {
        v10: derive(b"peanuts", 1),
        v11: secret.as_ref().ok().map(|secret| derive(secret, 1)),
        v11_failure: secret.err(),
        empty: Some(derive(b"", 1)),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn keys(_: &Chromium) -> Result<Keys, Failure> {
    Err(Failure::Unsupported)
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{collections::HashMap, time::Duration};

    use serde::de::DeserializeOwned;
    use zbus::{
        blocking::Connection,
        zvariant::{DynamicType, OwnedObjectPath, OwnedValue, Type, Value},
    };

    use super::Failure;

    const SERVICE: &str = "org.freedesktop.Secret.Service";
    const LOCKED: &str = "org.freedesktop.Secret.Error.IsLocked";

    pub(super) fn session() -> Result<Connection, Failure> {
        zbus::blocking::connection::Builder::session()
            // A keyring daemon that never answers must not hold the worker.
            .and_then(|builder| builder.method_timeout(Duration::from_secs(5)).build())
            .map_err(failure)
    }

    /// The secret of the first Secret Service item stored for `application`.
    pub(super) fn secret(bus: &Connection, application: &str) -> Result<Vec<u8>, Failure> {
        let service = "/org/freedesktop/secrets";
        // "plain" sends secrets unencrypted over the bus, which is local to
        // this user's session; Chromium negotiates the same.
        let (_, session): (OwnedValue, OwnedObjectPath) = call(
            bus,
            service,
            SERVICE,
            "OpenSession",
            &("plain", Value::from("")),
        )?;
        let (mut unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = call(
            bus,
            service,
            SERVICE,
            "SearchItems",
            &(HashMap::from([("application", application)]),),
        )?;
        if unlocked.is_empty() {
            if locked.is_empty() {
                return Err(Failure::KeyringMissing);
            }
            let prompt: OwnedObjectPath;
            (unlocked, prompt) = call(bus, service, SERVICE, "Unlock", &(locked,))?;
            // A prompt is the keyring asking for its password. Driving it
            // belongs to the user's desktop, so the import asks them instead.
            if prompt.as_str() != "/" || unlocked.is_empty() {
                return Err(Failure::KeyringLocked);
            }
        }
        // (session, parameters, value, content type)
        let (_, _, value, _): (OwnedObjectPath, Vec<u8>, Vec<u8>, String) = call(
            bus,
            unlocked[0].as_str(),
            "org.freedesktop.Secret.Item",
            "GetSecret",
            &(session,),
        )?;
        Ok(value)
    }

    fn call<B, R>(
        bus: &Connection,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<R, Failure>
    where
        B: serde::Serialize + DynamicType,
        R: DeserializeOwned + Type,
    {
        bus.call_method(
            Some("org.freedesktop.secrets"),
            path,
            Some(interface),
            method,
            body,
        )
        .and_then(|reply| reply.body().deserialize())
        .map_err(failure)
    }

    /// Never carries the error's text: a keyring error may name its items.
    fn failure(error: zbus::Error) -> Failure {
        match error {
            zbus::Error::MethodError(name, ..) if name.as_str() == LOCKED => Failure::KeyringLocked,
            _ => Failure::KeyringUnavailable,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{
            io::{BufRead, BufReader},
            process::{Child, Command, Stdio},
            sync::{Arc, Mutex},
        };
        use zbus::blocking::connection::Builder;

        struct PrivateBus(Child);

        impl Drop for PrivateBus {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        #[derive(Clone, Copy, PartialEq)]
        enum Keyring {
            Unlocked,
            /// Unlocks on request, as a keyring without a password does.
            Unlockable,
            /// Answers an unlock request with a prompt for the user.
            Prompting,
            /// Lists its item as unlocked but refuses to reveal it.
            Refusing,
        }

        const ITEM: &str = "/org/freedesktop/secrets/collection/login/1";
        const SESSION: &str = "/org/freedesktop/secrets/session/s1";

        fn path(path: &str) -> OwnedObjectPath {
            OwnedObjectPath::try_from(path).unwrap()
        }

        struct Service(Arc<Mutex<Keyring>>);

        #[zbus::interface(name = "org.freedesktop.Secret.Service")]
        impl Service {
            fn open_session(
                &self,
                algorithm: &str,
                _input: OwnedValue,
            ) -> zbus::fdo::Result<(OwnedValue, OwnedObjectPath)> {
                if algorithm != "plain" {
                    return Err(zbus::fdo::Error::NotSupported("algorithm".into()));
                }
                Ok((
                    OwnedValue::try_from(Value::from("")).unwrap(),
                    path(SESSION),
                ))
            }

            fn search_items(
                &self,
                attributes: HashMap<String, String>,
            ) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
                if attributes != HashMap::from([("application".into(), "chrome".into())]) {
                    return (Vec::new(), Vec::new());
                }
                match *self.0.lock().unwrap() {
                    Keyring::Unlocked | Keyring::Refusing => (vec![path(ITEM)], Vec::new()),
                    Keyring::Unlockable | Keyring::Prompting => (Vec::new(), vec![path(ITEM)]),
                }
            }

            fn unlock(
                &self,
                objects: Vec<OwnedObjectPath>,
            ) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
                let mut keyring = self.0.lock().unwrap();
                if *keyring == Keyring::Unlockable {
                    *keyring = Keyring::Unlocked;
                    (objects, path("/"))
                } else {
                    (Vec::new(), path("/org/freedesktop/secrets/prompt/p1"))
                }
            }
        }

        struct Item(Arc<Mutex<Keyring>>);

        #[zbus::interface(name = "org.freedesktop.Secret.Item")]
        impl Item {
            // One out argument that is a struct, hence the one-element tuple.
            #[allow(clippy::type_complexity)]
            fn get_secret(
                &self,
                session: OwnedObjectPath,
            ) -> Result<((OwnedObjectPath, Vec<u8>, Vec<u8>, String),), SecretError> {
                if session.as_str() != SESSION {
                    return Err(SecretError::NoSession(String::new()));
                }
                if *self.0.lock().unwrap() != Keyring::Unlocked {
                    return Err(SecretError::IsLocked(String::new()));
                }
                let secret = b"synthetic secret".to_vec();
                Ok(((session, Vec::new(), secret, "text/plain".into()),))
            }
        }

        #[derive(Debug, zbus::DBusError)]
        #[zbus(prefix = "org.freedesktop.Secret.Error")]
        enum SecretError {
            #[zbus(error)]
            ZBus(zbus::Error),
            IsLocked(String),
            NoSession(String),
        }

        #[test]
        fn secret_service_lookup_follows_the_keyring_state() {
            // A private daemon keeps the user's keyring out of the test; no
            // process-wide environment variables are changed. Its own
            // configuration names no service directory: the stock session
            // one would start the real keyring daemon for this bus name.
            let home = tempfile::tempdir().unwrap();
            let config = home.path().join("bus.conf");
            std::fs::write(
                &config,
                format!(
                    "<busconfig><type>session</type><listen>unix:dir={}</listen>\
                     <policy context=\"default\"><allow send_destination=\"*\"/>\
                     <allow receive_sender=\"*\"/><allow own=\"*\"/></policy></busconfig>",
                    home.path().display()
                ),
            )
            .unwrap();
            let mut daemon = PrivateBus(
                Command::new("dbus-daemon")
                    .arg(format!("--config-file={}", config.display()))
                    .args(["--nofork", "--print-address=1"])
                    .stdout(Stdio::piped())
                    .spawn()
                    .expect("the Secret Service test requires dbus-daemon"),
            );
            let mut address = String::new();
            BufReader::new(daemon.0.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            let address = address.trim();
            let client = Builder::address(address)
                .unwrap()
                .method_timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            // Nothing owns the service name yet.
            assert_eq!(secret(&client, "chrome"), Err(Failure::KeyringUnavailable));

            let keyring = Arc::new(Mutex::new(Keyring::Unlocked));
            let _server = Builder::address(address)
                .unwrap()
                .name("org.freedesktop.secrets")
                .unwrap()
                .serve_at("/org/freedesktop/secrets", Service(keyring.clone()))
                .unwrap()
                .serve_at(ITEM, Item(keyring.clone()))
                .unwrap()
                .build()
                .unwrap();
            let set = |state| *keyring.lock().unwrap() = state;

            assert_eq!(secret(&client, "chrome"), Ok(b"synthetic secret".to_vec()));
            assert_eq!(secret(&client, "brave"), Err(Failure::KeyringMissing));
            set(Keyring::Prompting);
            assert_eq!(secret(&client, "chrome"), Err(Failure::KeyringLocked));
            set(Keyring::Refusing);
            assert_eq!(secret(&client, "chrome"), Err(Failure::KeyringLocked));
            set(Keyring::Unlockable);
            assert_eq!(secret(&client, "chrome"), Ok(b"synthetic secret".to_vec()));
        }
    }
}
