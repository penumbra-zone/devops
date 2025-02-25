// use std::fs::File;
use std::path::PathBuf;
use std::process::Command;

use crate::PenumbraEnvironment;

/// Restore a PostgreSQL database dump to the target db, as defined by the `database_url`
/// connection string.
#[tracing::instrument]
pub fn restore_database(database_url: &str, dump_file: &PathBuf) -> anyhow::Result<()> {
    // pg_restore --exit-on-error --clean --if-exists \
    // --role penumbra --jobs "$(nproc)" --no-owner --no-acl \
    // -d "$DB_WRANGLER_LOCAL_SRC_DB_URL" "$DB_WRANGLER_COMETBFT_DUMP_LOCAL_FILEPATH"
    tracing::warn!(?database_url, "beginning restore");
    let status = Command::new("pg_restore")
        .args([
            "--exit-on-error",
            "--clean",
            "--if-exists",
            "--jobs",
            // TODO: figure out a dynamic value.
            "10",
            "--no-owner",
            "--no-acl",
            "-d",
            database_url,
            dump_file
                .to_str()
                .expect("failed to convert PathBuf to str"),
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("failed to restore database");
    }
}

/// Create a PostgreSQL database dump of the target db, as defined by the `database_url`
/// connection string. Will be formated as a "custom" pg dump, and saved to the local
/// filepath `dest_file`.
pub fn dump_database(database_url: &str, dest_file: &PathBuf) -> anyhow::Result<()> {
    let status = Command::new("pg_dump")
        .args([
            "-d",
            database_url,
            "-Fc",
            "-f",
            dest_file
                .to_str()
                .expect("failed to convert dump filepath to str"),
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!(format!(
            "failed to dump database to file: {}",
            dest_file.display()
        ));
    }
}

/// Look up a database URL for an online CometBFT event database.
/// Requires k8s access to look up the information from a k8s Secret.
/// This is a fallback, intended for use in PL infra tooling contexts.
/// Non-admin users can simply provide a `--src-db-url` on the CLI.
pub fn get_default_src_db_url(
    penumbra_environment: &PenumbraEnvironment,
) -> anyhow::Result<String> {
    use k8s_openapi::api::core::v1::Secret;
    let secret_name = format!("postgres-creds-{}-pindexer", penumbra_environment);
    let secret_field_name = "pindexer_src_database_url";

    tracing::debug!("looking up secret from k8s");
    let output = std::process::Command::new("kubectl")
        .args([
            "-n",
            penumbra_environment.to_string().as_str(),
            "get",
            "secret",
            &secret_name,
            "-o",
            "json",
        ])
        .output()?;

    if !output.status.success() {
        tracing::error!(?secret_name, "failed to find k8s secret");
        anyhow::bail!("secret lookup via k8s failed");
    }

    tracing::debug!("converting json output to k8s Secret");
    let secret: Secret = serde_json::from_slice(&output.stdout)?;
    let secret_data = secret.data.expect("k8s Secret must contain 'data' field");
    let database_url_bytes = secret_data
        .get(secret_field_name)
        .expect("secret lacks expected field name");
    let database_url = String::from_utf8_lossy(&database_url_bytes.0).to_string();

    Ok(database_url)
}
