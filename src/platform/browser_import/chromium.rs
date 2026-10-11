//! Chromium's cookie store: one SQLite table whose values are encrypted with
//! a key kept outside the profile.

use std::path::Path;

use aes::Aes128;
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use rusqlite::{Connection, OptionalExtension, Row, types::ValueRef};
use sha2::{Digest, Sha256};

use super::{Cookie, Failure, Read, SameSite};

pub(super) const COUNT: &str = "select count(*) from cookies";

/// Seconds from 1601, where Chromium's clock starts, to 1970.
const WINDOWS_TO_UNIX_EPOCH: i64 = 11_644_473_600;
/// From this store version on, each cookie may be bound to a top-level site.
const PARTITIONED_FROM: i64 = 15;
/// From this store version on, a value is preceded by the SHA-256 of its host.
const HOST_BOUND_FROM: i64 = 24;

pub(super) type Key = [u8; 16];

/// The keys a profile's values may be encrypted with, by blob prefix.
pub(super) struct Keys {
    pub v10: Key,
    pub v11: Option<Key>,
    /// Why there is no `v11` key; reported only if the read needed one.
    pub v11_failure: Option<Failure>,
    /// Linux builds once derived their key from an empty password by mistake.
    pub empty: Option<Key>,
}

pub(super) fn derive(password: &[u8], iterations: u32) -> Key {
    let mut key = Key::default();
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, b"saltysalt", iterations, &mut key);
    key
}

pub(super) fn read(db: &Path, keys: &Keys) -> Result<Read, Failure> {
    let snapshot = super::snapshot(db)?;
    let (read, needed_v11) = rows(&snapshot.db, keys).map_err(|_| Failure::ReadFailed)?;
    match keys.v11_failure {
        // Every usable cookie was behind the keyring: say so rather than
        // report an import of nothing.
        Some(failure) if needed_v11 && read.cookies.is_empty() => Err(failure),
        _ => Ok(read),
    }
}

/// The readable cookies, and whether any row was encrypted with the `v11` key.
fn rows(db: &Connection, keys: &Keys) -> rusqlite::Result<(Read, bool)> {
    let version = db
        .query_row(
            "select value from meta where key = 'version' limit 1",
            [],
            |row| {
                Ok(match row.get_ref(0)? {
                    ValueRef::Integer(version) => version,
                    ValueRef::Text(text) => std::str::from_utf8(text)
                        .ok()
                        .and_then(|text| text.trim().parse().ok())
                        .unwrap_or(0),
                    _ => 0,
                })
            },
        )
        .optional()?
        .unwrap_or(0);
    let site = if version >= PARTITIONED_FROM {
        "top_frame_site_key"
    } else {
        "'' as top_frame_site_key"
    };
    let mut statement = db.prepare(&format!(
        "select host_key, name, value, encrypted_value, path, \
         expires_utc / 1000000 as expires_seconds, is_secure, is_httponly, samesite, {site} \
         from cookies"
    ))?;
    let mut rows = statement.query([])?;
    let mut read = Read::default();
    let mut needed_v11 = false;
    while let Some(row) = rows.next()? {
        if read.full() {
            read.skipped += 1;
            continue;
        }
        // A row of unexpected types is skipped like one that fails to decrypt.
        match cookie(row, version, keys, &mut needed_v11) {
            Ok(Some(cookie)) => read.push(cookie),
            _ => read.skipped += 1,
        }
    }
    Ok((read, needed_v11))
}

fn cookie(
    row: &Row,
    version: i64,
    keys: &Keys,
    needed_v11: &mut bool,
) -> rusqlite::Result<Option<Cookie>> {
    // A partitioned cookie belongs to one embedding site, which an import
    // into another browser's unpartitioned jar would not preserve.
    let site: Option<String> = row.get(9)?;
    if site.is_some_and(|site| !site.is_empty()) {
        return Ok(None);
    }
    let host: String = row.get(0)?;
    let encrypted: Option<Vec<u8>> = row.get(3)?;
    let value = match encrypted.filter(|blob| !blob.is_empty()) {
        None => row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        Some(blob) => {
            *needed_v11 |= blob.starts_with(b"v11");
            match decrypt(&blob, &host, version, keys) {
                Some(value) => value,
                None => return Ok(None),
            }
        }
    };
    let flag = |column| {
        Ok::<_, rusqlite::Error>(
            row.get::<_, Option<i64>>(column)?
                .is_some_and(|flag| flag != 0),
        )
    };
    let expires: i64 = row.get::<_, Option<i64>>(5)?.unwrap_or(0);
    Ok(Some(Cookie {
        name: row.get(1)?,
        value,
        path: row.get(4)?,
        secure: flag(6)?,
        http_only: flag(7)?,
        same_site: SameSite::stored(row.get(8)?),
        expires: (expires > 0).then(|| expires - WINDOWS_TO_UNIX_EPOCH),
        host,
    }))
}

fn decrypt(blob: &[u8], host: &str, version: i64, keys: &Keys) -> Option<String> {
    let key = match blob.get(..3) {
        Some(b"v10") => keys.v10,
        Some(b"v11") => keys.v11?,
        // Stores older than value encryption hold the text itself.
        _ => return String::from_utf8(blob.to_vec()).ok(),
    };
    let mut plain = aes_cbc(&key, &blob[3..])
        .or_else(|| keys.empty.and_then(|empty| aes_cbc(&empty, &blob[3..])))?;
    if version >= HOST_BOUND_FROM {
        // The host exactly as stored, leading dot included.
        let digest = Sha256::digest(host.as_bytes());
        if plain.get(..digest.len()) != Some(digest.as_slice()) {
            return None;
        }
        plain.drain(..digest.len());
    }
    String::from_utf8(plain).ok()
}

/// AES-128-CBC with PKCS#7 padding under Chromium's fixed IV of sixteen spaces.
fn aes_cbc(key: &Key, data: &[u8]) -> Option<Vec<u8>> {
    let mut buffer = data.to_vec();
    let length = cbc::Decryptor::<Aes128>::new(key.into(), &[b' '; 16].into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .ok()?
        .len();
    buffer.truncate(length);
    Some(buffer)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use cbc::cipher::BlockEncryptMut;
    use rusqlite::params;

    const V11: Key = *b"a known v11 key!";

    fn keys() -> Keys {
        Keys {
            v10: derive(b"peanuts", 1),
            v11: Some(V11),
            v11_failure: None,
            empty: Some(derive(b"", 1)),
        }
    }

    /// An empty store of the given version, as Chromium lays it out.
    pub(in super::super) fn database(path: &Path, version: i64) -> Connection {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Connection::open(path).unwrap();
        let site = if version >= PARTITIONED_FROM {
            ", top_frame_site_key text not null default ''"
        } else {
            ""
        };
        db.execute_batch(&format!(
            "create table meta (key longvarchar not null unique primary key, value longvarchar);
             insert into meta values ('version', '{version}'), ('last_compatible_version', '{version}');
             create table cookies (creation_utc integer not null, host_key text not null,
             name text not null, value text not null, encrypted_value blob not null,
             path text not null, expires_utc integer not null, is_secure integer not null,
             is_httponly integer not null, samesite integer not null{site});"
        ))
        .unwrap();
        db
    }

    fn encrypt(prefix: &[u8], key: &Key, plain: &[u8]) -> Vec<u8> {
        let mut buffer = plain.to_vec();
        buffer.resize(plain.len() + 16, 0);
        let length = cbc::Encryptor::<Aes128>::new(key.into(), &[b' '; 16].into())
            .encrypt_padded_mut::<Pkcs7>(&mut buffer, plain.len())
            .unwrap()
            .len();
        buffer.truncate(length);
        [prefix, &buffer].concat()
    }

    /// What Chromium encrypts for `value` at a store version.
    fn bound(version: i64, host: &str, value: &[u8]) -> Vec<u8> {
        let mut plain = Vec::new();
        if version >= HOST_BOUND_FROM {
            plain.extend_from_slice(&Sha256::digest(host.as_bytes()));
        }
        plain.extend_from_slice(value);
        plain
    }

    fn insert(db: &Connection, host: &str, name: &str, value: &str, encrypted: &[u8]) {
        db.execute(
            "insert into cookies (creation_utc, host_key, name, value, encrypted_value, path,
             expires_utc, is_secure, is_httponly, samesite) values (0, ?1, ?2, ?3, ?4, '/', 0, 0, 0, -1)",
            params![host, name, value, encrypted],
        )
        .unwrap();
    }

    fn values(read: &Read) -> Vec<(&str, &str)> {
        read.cookies
            .iter()
            .map(|cookie| (cookie.name.as_str(), cookie.value.as_str()))
            .collect()
    }

    #[test]
    fn keys_are_derived_as_chromium_derives_them() {
        // Chromium's own constants for the Linux fallback keys.
        assert_eq!(
            derive(b"peanuts", 1),
            [
                0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27,
                0x27, 0x78
            ]
        );
        assert_eq!(
            derive(b"", 1),
            [
                0xd0, 0xd0, 0xec, 0x9c, 0x7d, 0x77, 0xd4, 0x3a, 0xc5, 0x41, 0x87, 0xfa, 0x48, 0x18,
                0xd1, 0x7f
            ]
        );
        // The macOS iteration count, against an independent implementation.
        assert_eq!(
            derive(b"secret", 1003),
            [
                0x1a, 0x74, 0x04, 0x70, 0x4e, 0xe3, 0x5b, 0x45, 0x06, 0x62, 0x4e, 0xd4, 0x91, 0x71,
                0xa5, 0x34
            ]
        );
    }

    #[test]
    fn every_store_version_reads_each_kind_of_value() {
        let keys = keys();
        for version in [14, 18, 24] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("Cookies");
            let db = database(&path, version);
            let host = ".example.test";
            let sealed = |prefix: &[u8], key: &Key, value: &[u8]| {
                encrypt(prefix, key, &bound(version, host, value))
            };
            insert(&db, host, "v10", "", &sealed(b"v10", &keys.v10, b"peanuts"));
            insert(&db, host, "v11", "", &sealed(b"v11", &V11, b"keyring"));
            insert(&db, host, "plain", "in the value column", b"");
            insert(&db, host, "legacy", "", b"before encryption");
            insert(
                &db,
                host,
                "empty-key",
                "",
                &sealed(b"v10", &keys.empty.unwrap(), b"empty password"),
            );
            insert(
                &db,
                host,
                "empty-value",
                "",
                &sealed(b"v10", &keys.v10, b""),
            );
            // Skipped: not text, not this key, not a whole block.
            insert(
                &db,
                host,
                "binary",
                "",
                &sealed(b"v10", &keys.v10, b"\xff\xfe"),
            );
            insert(&db, host, "binary-legacy", "", b"\xff\xfe\xfd\xfc");
            insert(
                &db,
                host,
                "foreign",
                "",
                &sealed(b"v10", b"someone else key", b"x"),
            );
            insert(&db, host, "truncated", "", b"v10short");
            // The digest is of another host: skipped once stores bind values.
            let other = encrypt(b"v10", &keys.v10, &bound(version, "example.test", b"moved"));
            insert(&db, host, "wrong-host", "", &other);
            let mut expected = vec![
                ("v10", "peanuts"),
                ("v11", "keyring"),
                ("plain", "in the value column"),
                ("legacy", "before encryption"),
                ("empty-key", "empty password"),
                ("empty-value", ""),
            ];
            let mut skipped = 4;
            if version >= HOST_BOUND_FROM {
                skipped += 1;
            } else {
                expected.push(("wrong-host", "moved"));
            }
            if version >= PARTITIONED_FROM {
                db.execute(
                    "insert into cookies values (0, ?1, 'partitioned', 'p', x'', '/', 0, 0, 0, -1,
                     'https://embedder.test')",
                    [host],
                )
                .unwrap();
                skipped += 1;
            }
            drop(db);

            let read = read(&path, &keys).unwrap();
            assert_eq!(values(&read), expected, "version {version}");
            assert_eq!(read.skipped, skipped, "version {version}");
        }
    }

    #[test]
    fn cookie_attributes_are_mapped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Cookies");
        let db = database(&path, 24);
        // The version is stored as text by Chromium; an integer reads alike.
        db.execute("update meta set value = 24 where key = 'version'", [])
            .unwrap();
        let unix = 1_900_000_000_i64;
        let chromium = (unix + WINDOWS_TO_UNIX_EPOCH) * 1_000_000 + 999_999;
        for (name, expires, secure, http_only, same_site) in [
            ("a", chromium, 1, 0, 0),
            ("b", 0, 0, 1, 1),
            ("c", -5, 1, 1, 2),
            ("d", 0, 0, 0, -1),
            ("e", 0, 0, 0, 7),
        ] {
            db.execute(
                "insert into cookies values (0, 'host.test', ?1, 'v', x'', '/p', ?2, ?3, ?4, ?5, '')",
                params![name, expires, secure, http_only, same_site],
            )
            .unwrap();
        }
        drop(db);

        let read = read(&path, &keys()).unwrap();
        let attributes: Vec<_> = read
            .cookies
            .iter()
            .map(|c| (c.expires, c.secure, c.http_only, c.same_site))
            .collect();
        assert_eq!(
            attributes,
            [
                (Some(unix), true, false, SameSite::None),
                (None, false, true, SameSite::Lax),
                (None, true, true, SameSite::Strict),
                (None, false, false, SameSite::Unspecified),
                (None, false, false, SameSite::Unspecified),
            ]
        );
        assert_eq!(read.cookies[0].host, "host.test");
        assert_eq!(read.cookies[0].path, "/p");
    }

    #[test]
    fn a_missing_v11_key_fails_only_a_read_that_needed_it() {
        let without = Keys {
            v11: None,
            v11_failure: Some(Failure::KeyringLocked),
            ..keys()
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Cookies");
        let db = database(&path, 18);
        let host = "example.test";
        db.execute(
            "insert into cookies values (0, ?1, 'partitioned', '', ?2, '/', 0, 0, 0, -1, 'https://e.test')",
            params![host, encrypt(b"v11", &V11, b"x")],
        )
        .unwrap();
        // A partitioned row is skipped whatever its key, so none was needed.
        let outcome = read(&path, &without).unwrap();
        assert_eq!((outcome.cookies.len(), outcome.skipped), (0, 1));

        insert(&db, host, "v11", "", &encrypt(b"v11", &V11, b"keyring"));
        assert_eq!(read(&path, &without).err(), Some(Failure::KeyringLocked));
        // No failure to report: the rows are skipped.
        let silent = Keys {
            v11: None,
            v11_failure: None,
            ..keys()
        };
        assert_eq!(read(&path, &silent).unwrap().skipped, 2);

        insert(&db, host, "v10", "", &encrypt(b"v10", &without.v10, b"ok"));
        let outcome = read(&path, &without).unwrap();
        assert_eq!(values(&outcome), [("v10", "ok")]);
        assert_eq!(outcome.skipped, 2);
        assert_eq!(values(&read(&path, &keys()).unwrap()).len(), 2);
    }

    #[test]
    fn a_store_without_the_expected_tables_fails() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Cookies");
        Connection::open(&path)
            .unwrap()
            .execute_batch("create table other (x)")
            .unwrap();
        assert_eq!(read(&path, &keys()).err(), Some(Failure::ReadFailed));
    }

    #[test]
    fn the_source_database_is_left_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Cookies");
        let db = database(&path, 24);
        insert(&db, "example.test", "a", "v", b"");
        drop(db);
        let before = std::fs::read(&path).unwrap();
        read(&path, &keys()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let entries: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["Cookies"]);
    }
}
