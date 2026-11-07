use std::{io, time::Duration};

pub fn env_value<T: core::str::FromStr>(prefix: &str, name: &str) -> io::Result<Option<T>>
where
    T::Err: std::fmt::Display,
{
    let Ok(value) = std::env::var(format!("UPSTREAM_{prefix}_{name}")) else {
        return Ok(None);
    };
    let value = value
        .parse()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{err}")))?;
    Ok(Some(value))
}

pub fn env_value_with_default<T: core::str::FromStr>(
    prefix: &str,
    name: &str,
    default: T,
) -> io::Result<T>
where
    T::Err: std::fmt::Display,
{
    Ok(env_value(prefix, name)?.unwrap_or(default))
}

pub fn target_latency() -> io::Result<Duration> {
    let mills = env_value_with_default("SERVER", "TARGET_QUEUE_DELAY_MS", 10)?;
    Ok(Duration::from_millis(mills))
}
