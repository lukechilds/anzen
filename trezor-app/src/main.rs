#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod benchmark;
mod protocol;

use benchmark::{benchmark_fixed_digest_signing, benchmark_graph, benchmark_key_derivation};
use protocol::{BENCHMARK_RESULT, BenchmarkResponse, RUN_BENCHMARK};
use trezor_app_sdk::{
    Error, Result, ResultExt,
    ui::{self, ConfirmProperties, Property, ShowSuccess, TrezorUiResult},
    util::SliceWriter,
    wire_error_raw, wire_receive_wire_start, wire_respond_raw,
};
use ufmt::uwrite;

fn rounded_millis(microseconds: u64) -> Result<u32> {
    u32::try_from((microseconds + 500) / 1_000)
        .map_err(|_| Error::DataError("Benchmark duration overflow"))
}

fn review_policy() -> Result<()> {
    let properties = [
        Property::plain("Vault balance", "2.1 BTC"),
        Property::plain("Monthly allowance", "0.1 BTC"),
        Property::plain("Emergency access", "0.5 BTC"),
    ];
    match ui::confirm_properties(ConfirmProperties::new(
        "Anzen annual policy",
        &properties,
        None,
        Some("Approve policy"),
        true,
        Some("anzen_benchmark_policy"),
        1,
    ))? {
        TrezorUiResult::Confirmed => Ok(()),
        _ => Err(Error::Cancelled),
    }
}

fn execute_benchmark() -> Result<BenchmarkResponse> {
    review_policy()?;
    ui::init_progress(
        Some("Deriving benchmark key"),
        Some("Preparing deterministic vault"),
        true,
        false,
    )?;

    let benchmark: Result<BenchmarkResponse> = (|| {
        let key = benchmark_key_derivation()?;
        ui::update_progress(Some("Constructing transaction graph"), 25)?;
        let graph = benchmark_graph(key.commitment)?;
        ui::update_progress(Some("Signing annual policy"), 60)?;
        let signing = benchmark_fixed_digest_signing(graph.summary.signature_jobs)?;
        ui::update_progress(Some("Finishing benchmark"), 100)?;

        let key_derivation_ms = rounded_millis(key.elapsed_us)?;
        let graph_ms = rounded_millis(graph.elapsed_us)?;
        let signing_ms = rounded_millis(signing.elapsed_us)?;
        let total_ms = key_derivation_ms
            .checked_add(graph_ms)
            .and_then(|value| value.checked_add(signing_ms))
            .ok_or(Error::DataError("Benchmark total overflow"))?;

        Ok(BenchmarkResponse {
            transactions: graph.summary.transactions,
            signatures: graph.summary.signature_jobs,
            key_derivation_ms,
            graph_ms,
            signing_ms,
            total_ms,
            last_sighash: graph.last_sighash,
            last_signature_r: signing.commitment,
        })
    })();

    ui::end_progress()?;
    let response = benchmark?;

    let mut content_buffer = [0_u8; 192];
    let mut content = SliceWriter::new(&mut content_buffer);
    uwrite!(
        content,
        "Key derivation: {} ms\nGraph: {} ms\n{} signatures: {} ms\nFull workload: {} ms",
        response.key_derivation_ms,
        response.graph_ms,
        response.signatures,
        response.signing_ms,
        response.total_ms
    )
    .map_err(|_| Error::DataError("Benchmark result text overflow"))?;
    ui::show_success(ShowSuccess::new(
        "Anzen",
        content.as_ref(),
        "Continue",
        None,
        Some("anzen_benchmark_complete"),
        1,
    ))?;
    Ok(response)
}

fn handle_message(message_id: u16, data: &[u8]) -> Result<()> {
    if message_id != RUN_BENCHMARK {
        return Err(Error::InvalidMessage);
    }
    protocol::decode_request(data)?;
    let response = execute_benchmark()?.encode();
    wire_respond_raw(BENCHMARK_RESULT.into(), &response)
}

#[unsafe(no_mangle)]
pub fn app() -> Result<()> {
    loop {
        let (message_id, data) = wire_receive_wire_start().c()?;
        if let Err(error) = handle_message(message_id, &data) {
            wire_error_raw(&error).c()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_rounding_matches_the_embedded_benchmark() {
        assert_eq!(rounded_millis(0).unwrap(), 0);
        assert_eq!(rounded_millis(499).unwrap(), 0);
        assert_eq!(rounded_millis(500).unwrap(), 1);
        assert_eq!(rounded_millis(1_499).unwrap(), 1);
        assert_eq!(rounded_millis(1_500).unwrap(), 2);
    }
}
