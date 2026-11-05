//! Environment variable retrieval (wasi: the process environment supplied by the MavexOS kernel).

pub(crate) fn get_host_env_vars() -> impl Iterator<Item = (String, String)> {
    std::env::vars()
}
