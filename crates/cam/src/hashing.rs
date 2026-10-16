//! Incremental encoding of the existing snapshot fingerprint, without a full JSON buffer.
use crate::{Error, JobInput, Result, VERSION, invalid};
use serde::Serialize;
use sha2::{Digest, Sha256};

struct Encoder(Sha256);
impl Encoder {
    fn bytes(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
    fn value(&mut self, value: &impl Serialize) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|e| Error(e.to_string()))?;
        self.bytes(&bytes);
        Ok(())
    }
    async fn array<T: Serialize, F: std::future::Future<Output = ()>>(
        &mut self,
        values: &[T],
        batch: usize,
        cancelled: &impl Fn() -> bool,
        yield_control: &mut impl FnMut() -> F,
    ) -> Result<()> {
        self.bytes(b"[");
        for (index, chunk) in values.chunks(batch).enumerate() {
            checkpoint(cancelled, yield_control).await?;
            if index != 0 {
                self.bytes(b",");
            }
            // Serialize each bounded slice using Serde's own number/string rules.
            // Only its enclosing brackets are omitted when joining array chunks.
            let bytes = serde_json::to_vec(chunk).map_err(|e| Error(e.to_string()))?;
            let contents = bytes.get(1..bytes.len().saturating_sub(1)).ok_or_else(|| Error("invalid fingerprint array encoding".into()))?;
            self.bytes(contents);
        }
        self.bytes(b"]");
        Ok(())
    }
}
async fn checkpoint<F: std::future::Future<Output = ()>>(cancelled: &impl Fn() -> bool, yield_control: &mut impl FnMut() -> F) -> Result<()> {
    if cancelled() {
        return invalid("fingerprint cancelled");
    }
    yield_control().await;
    if cancelled() { invalid("fingerprint cancelled") } else { Ok(()) }
}

/// Hash a validated immutable job using exactly the bytes of `fingerprint(input)`.
/// Yields before each operation/tool and every 256 triangles. Encoding one setup
/// or operation remains atomic; this is not a hard elapsed-time guarantee.
/// Cancellation returns no digest and never mutates the input.
pub async fn fingerprint_job_cooperative<F: std::future::Future<Output = ()>>(
    input: &JobInput,
    cancelled: impl Fn() -> bool,
    mut yield_control: impl FnMut() -> F,
) -> Result<String> {
    checkpoint(&cancelled, &mut yield_control).await?;
    let mut encoder = Encoder(Sha256::new());
    encoder.bytes(b"[");
    encoder.value(&VERSION)?;
    // Field order deliberately matches the derived JobInput/Geometry encoders.
    // Equality regressions below make schema additions or reorderings visible.
    encoder.bytes(b",{\"setup\":");
    encoder.value(&input.setup)?;
    encoder.bytes(b",\"tools\":");
    encoder.array(&input.tools, 1, &cancelled, &mut yield_control).await?;
    encoder.bytes(b",\"operations\":");
    encoder.array(&input.operations, 1, &cancelled, &mut yield_control).await?;
    encoder.bytes(b",\"geometry\":{\"target\":");
    encoder.array(&input.geometry.target, 256, &cancelled, &mut yield_control).await?;
    encoder.bytes(b",\"fixtures\":");
    encoder.array(&input.geometry.fixtures, 256, &cancelled, &mut yield_control).await?;
    encoder.bytes(b",\"fixture_groups\":");
    encoder.array(&input.geometry.fixture_groups, 256, &cancelled, &mut yield_control).await?;
    encoder.bytes(b"},\"fingerprint\":");
    encoder.value(&input.fingerprint)?;
    encoder.bytes(b"}]");
    checkpoint(&cancelled, &mut yield_control).await?;
    Ok(format!("{:x}", encoder.0.finalize()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        future::Future,
        task::{Context, Poll, Waker},
    };

    fn ready<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test checkpoint unexpectedly suspended"),
        }
    }
    #[test]
    fn cooperative_encoding_preserves_existing_snapshot_fingerprints() {
        for name in crate::corpus::CASES {
            let mut input = crate::corpus::fixture(name).unwrap();
            input.setup.name = "Escaped \"setup\" \\ \nμ".into();
            input.fingerprint = "Existing digest remains part of the serialized input".into();
            let expected = crate::fingerprint(&input).unwrap();
            let count = Cell::new(0);
            let actual = ready(fingerprint_job_cooperative(
                &input,
                || false,
                || {
                    count.set(count.get() + 1);
                    std::future::ready(())
                },
            ))
            .unwrap();
            assert_eq!(actual, expected, "{name}");
            assert!(count.get() >= 4);
        }
    }
    #[test]
    fn large_snapshot_hashing_yields_and_cancellation_never_returns_a_digest() {
        let mut input = crate::corpus::fixture("face").unwrap();
        input.geometry.target.resize(200_000, input.geometry.target[0]);
        let expected = crate::fingerprint(&input).unwrap();
        let count = Cell::new(0);
        let actual = ready(fingerprint_job_cooperative(
            &input,
            || false,
            || {
                count.set(count.get() + 1);
                std::future::ready(())
            },
        ))
        .unwrap();
        assert_eq!(actual, expected);
        assert!(count.get() > 780);
        for stop in [0, 1, 400, count.get()] {
            let checkpoints = Cell::new(0);
            let result = ready(fingerprint_job_cooperative(
                &input,
                || checkpoints.get() >= stop,
                || {
                    checkpoints.set(checkpoints.get() + 1);
                    std::future::ready(())
                },
            ));
            assert!(result.is_err(), "cancel at checkpoint {stop}");
        }
        assert_eq!(crate::fingerprint(&input).unwrap(), expected);
    }
}
