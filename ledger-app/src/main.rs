#![no_std]
#![no_main]

extern crate alloc;

mod benchmark;

use alloc::format;
use anzen_cold_signer::{PROTOCOL_NAME, benchmark::WorkloadSummary};
use benchmark::{BenchmarkContext, BenchmarkError};
use core::sync::atomic::{AtomicBool, Ordering};
use ledger_device_sdk::include_gif;
use ledger_device_sdk::io::{ApduHeader, Comm, Command, DecodedEventType, Reply, StatusWords};
use ledger_device_sdk::nbgl::{
    Field, InfoLongPress, NbglAction, NbglGenericReview, NbglGlyph, NbglPageContent, NbglSpinner,
    NbglStatus, TagValueList, TuneIndex, init_comm,
};
use ledger_device_sdk::sys;

ledger_device_sdk::set_panic!(ledger_device_sdk::exiting_panic);
ledger_device_sdk::define_comm!(COMM);

const APP_NAME: &str = PROTOCOL_NAME;
const APP_TAGLINE: &str = "Cold storage made easy";
const BENCHMARK_CLA: u8 = 0xe0;
const PREPARE_INS: u8 = 0x20;
const KEY_DERIVATION_INS: u8 = 0x21;
const GRAPH_INS: u8 = 0x22;
const SIGNING_INS: u8 = 0x23;
const COMPLETE_INS: u8 = 0x24;
const BENCHMARK_VERSION: u8 = 3;

#[cfg(target_os = "apex_p")]
const APP_ICON: NbglGlyph = NbglGlyph::from_include(include_gif!("icons/anzen-32x32.png", NBGL));
#[cfg(any(target_os = "stax", target_os = "flex"))]
const APP_ICON: NbglGlyph = NbglGlyph::from_include(include_gif!("icons/anzen-64x64.png", NBGL));
#[cfg(any(target_os = "nanosplus", target_os = "nanox"))]
const APP_ICON: NbglGlyph = NbglGlyph::from_include(include_gif!("icons/anzen-14x14.png", NBGL));

static START_LOCAL_BENCHMARK: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn start_local_benchmark() {
    START_LOCAL_BENCHMARK.store(true, Ordering::Release);
}

unsafe extern "C" fn quit_app() {
    ledger_device_sdk::exit_app(0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchmarkInstruction {
    Prepare { rollover_inputs: u8 },
    KeyDerivation,
    Graph,
    Signing,
    Complete,
}

impl TryFrom<ApduHeader> for BenchmarkInstruction {
    type Error = StatusWords;

    fn try_from(header: ApduHeader) -> Result<Self, Self::Error> {
        if header.cla != BENCHMARK_CLA {
            return Err(StatusWords::BadCla);
        }
        match (header.ins, header.p1, header.p2) {
            (PREPARE_INS, inputs @ (1 | 2 | 12), 0) => Ok(Self::Prepare {
                rollover_inputs: inputs,
            }),
            (KEY_DERIVATION_INS, 0, 0) => Ok(Self::KeyDerivation),
            (GRAPH_INS, 0, 0) => Ok(Self::Graph),
            (SIGNING_INS, 0, 0) => Ok(Self::Signing),
            (COMPLETE_INS, 0, 0) => Ok(Self::Complete),
            (PREPARE_INS | KEY_DERIVATION_INS | GRAPH_INS | SIGNING_INS | COMPLETE_INS, _, _) => {
                Err(StatusWords::BadP1P2)
            }
            _ => Err(StatusWords::BadIns),
        }
    }
}

/// Show Ledger's standard home-and-settings use case with a primary action. APDUs are also
/// accepted while this screen is active so the host benchmark can initiate the same review.
fn show_home(comm: &mut Comm) {
    let icon: sys::nbgl_icon_details_t = (&APP_ICON).into();
    let info_names = [c"Version".as_ptr(), c"Developer".as_ptr()];
    let info_values = [
        c"0.1.0 signing benchmark".as_ptr(),
        c"Anzen developers".as_ptr(),
    ];
    let info_list = sys::nbgl_contentInfoList_t {
        infoTypes: info_names.as_ptr(),
        infoContents: info_values.as_ptr(),
        nbInfos: info_names.len() as u8,
        ..Default::default()
    };
    let action = sys::nbgl_homeAction_t {
        text: c"Run signing benchmark".as_ptr(),
        icon: core::ptr::null(),
        callback: Some(start_local_benchmark),
        style: sys::STRONG_HOME_ACTION,
    };

    START_LOCAL_BENCHMARK.store(false, Ordering::Release);
    unsafe {
        sys::nbgl_useCaseHomeAndSettings(
            c"Anzen".as_ptr(),
            &icon,
            c"Cold storage made easy".as_ptr(),
            sys::INIT_HOME_PAGE as u8,
            core::ptr::null(),
            &info_list,
            &action,
            Some(quit_app),
        );
    }

    loop {
        if START_LOCAL_BENCHMARK.load(Ordering::Acquire) {
            run_local_benchmark(comm);
            return;
        }
        match comm.try_next_event().into_type() {
            DecodedEventType::Apdu {
                header,
                offset,
                length,
            } => {
                let command = Command::new(comm, header, offset, length);
                handle_host_command(command);
                return;
            }
            DecodedEventType::ApduError(_) => {
                let _ = comm.send(&[], StatusWords::BadLen);
            }
            _ => {}
        }
    }
}

fn run_local_benchmark(comm: &mut Comm) {
    let mut spinner = NbglSpinner::new();
    spinner.show("Preparing deterministic vault");
    let context = match BenchmarkContext::prepare(12) {
        Ok(context) => context,
        Err(_) => {
            NbglStatus::new()
                .text("Benchmark preparation failed")
                .show(comm, false);
            return;
        }
    };
    if !review_benchmark_policy(comm) {
        NbglStatus::new()
            .text("Benchmark cancelled")
            .show(comm, false);
        return;
    }
    spinner.show("Deriving benchmark key");
    if let Err(error) = context.benchmark_key_derivation() {
        show_local_run_error(comm, error);
        return;
    }
    spinner.show("Building transaction graph");
    if let Err(error) = context.benchmark_graph() {
        show_local_run_error(comm, error);
        return;
    }
    spinner.show("Signing fixed digest");
    if let Err(error) = context.benchmark_fixed_digest_signing() {
        show_local_run_error(comm, error);
        return;
    }
    show_benchmark_complete(comm, context.summary().signature_jobs, None);
}

fn show_local_run_error(comm: &mut Comm, error: BenchmarkError) {
    let message = match error {
        BenchmarkError::SignatureCreation => "Hardware signing failed",
        BenchmarkError::PublicKeyMismatch => "Derived benchmark key changed",
        BenchmarkError::SignatureCountMismatch => "Signature count check failed",
        BenchmarkError::Graph(_) => "Transaction graph failed",
        BenchmarkError::Crypto => "Ledger cryptography failed",
    };
    NbglStatus::new().text(message).show(comm, false);
}

fn handle_host_command(command: Command<'_>) {
    if !command.get_data().is_empty() {
        let _ = command.reply(&[], StatusWords::BadLen);
        return;
    }
    let instruction = match command.decode::<BenchmarkInstruction>() {
        Ok(instruction) => instruction,
        Err(reply) => {
            let _ = command.reply(&[], reply);
            return;
        }
    };
    let rollover_inputs = match instruction {
        BenchmarkInstruction::Prepare { rollover_inputs } => rollover_inputs,
        _ => {
            let _ = command.reply(&[], StatusWords::BadIns);
            return;
        }
    };
    let comm = command.into_comm();
    let mut spinner = NbglSpinner::new();
    spinner.show("Preparing deterministic vault");
    let context = match BenchmarkContext::prepare(rollover_inputs) {
        Ok(context) => context,
        Err(error) => {
            let _ = comm.send(&[], benchmark_failure_reply(error));
            return;
        }
    };
    if !review_benchmark_policy(comm) {
        let _ = comm.send(&[], StatusWords::UserCancelled);
        return;
    }

    let summary = context.summary();
    let public_key = context.hww_xonly_public_key();
    let prepared_response = workload_response(summary, public_key);
    if comm.send(&prepared_response, StatusWords::Ok).is_err() {
        return;
    }

    spinner.show("Ready for key derivation");
    let derivation_command = match next_expected_command(comm, BenchmarkInstruction::KeyDerivation)
    {
        Some(command) => command,
        None => return,
    };
    if !derivation_command.get_data().is_empty() {
        let _ = derivation_command.reply(&[], StatusWords::BadLen);
        return;
    }
    spinner.show("Deriving benchmark key");
    let derived_public_key = match context.benchmark_key_derivation() {
        Ok(public_key) => public_key,
        Err(error) => {
            let _ = derivation_command.reply(&[], benchmark_failure_reply(error));
            return;
        }
    };
    let derivation_response = workload_response(summary, derived_public_key);
    let comm = match derivation_command
        .into_response()
        .extend(&derivation_response)
        .and_then(|response| response.send(StatusWords::Ok).map(|comm| comm))
    {
        Ok(comm) => comm,
        Err(_) => return,
    };

    spinner.show("Ready for transaction graph");
    let graph_command = match next_expected_command(comm, BenchmarkInstruction::Graph) {
        Some(command) => command,
        None => return,
    };
    if !graph_command.get_data().is_empty() {
        let _ = graph_command.reply(&[], StatusWords::BadLen);
        return;
    }
    spinner.show("Building transaction graph");
    let last_sighash = match context.benchmark_graph() {
        Ok(sighash) => sighash,
        Err(error) => {
            let _ = graph_command.reply(&[], benchmark_failure_reply(error));
            return;
        }
    };
    let graph_response = workload_response(summary, last_sighash);
    let comm = match graph_command
        .into_response()
        .extend(&graph_response)
        .and_then(|response| response.send(StatusWords::Ok).map(|comm| comm))
    {
        Ok(comm) => comm,
        Err(_) => return,
    };

    spinner.show("Ready for fixed-digest signing");
    let signing_command = match next_expected_command(comm, BenchmarkInstruction::Signing) {
        Some(command) => command,
        None => return,
    };
    if !signing_command.get_data().is_empty() {
        let _ = signing_command.reply(&[], StatusWords::BadLen);
        return;
    }
    spinner.show("Signing fixed digest");
    let last_signature_r = match context.benchmark_fixed_digest_signing() {
        Ok(signature_r) => signature_r,
        Err(error) => {
            let _ = signing_command.reply(&[], benchmark_failure_reply(error));
            return;
        }
    };
    let signing_response = workload_response(summary, last_signature_r);
    let comm = match signing_command
        .into_response()
        .extend(&signing_response)
        .and_then(|response| response.send(StatusWords::Ok).map(|comm| comm))
    {
        Ok(comm) => comm,
        Err(_) => return,
    };

    spinner.show("Finishing benchmark");
    let complete_command = match next_expected_command(comm, BenchmarkInstruction::Complete) {
        Some(command) => command,
        None => return,
    };
    let encoded_timings: [u8; 12] = match complete_command.get_data().try_into() {
        Ok(encoded) => encoded,
        Err(_) => {
            let _ = complete_command.reply(&[], StatusWords::BadLen);
            return;
        }
    };
    let timings = BenchmarkTimings {
        key_derivation_ms: u32::from_be_bytes([
            encoded_timings[0],
            encoded_timings[1],
            encoded_timings[2],
            encoded_timings[3],
        ]),
        graph_ms: u32::from_be_bytes([
            encoded_timings[4],
            encoded_timings[5],
            encoded_timings[6],
            encoded_timings[7],
        ]),
        signing_ms: u32::from_be_bytes([
            encoded_timings[8],
            encoded_timings[9],
            encoded_timings[10],
            encoded_timings[11],
        ]),
    };
    if complete_command
        .reply(&[BENCHMARK_VERSION], StatusWords::Ok)
        .is_err()
    {
        return;
    }
    show_benchmark_complete(comm, summary.signature_jobs, Some(timings));
}

fn workload_response(summary: WorkloadSummary, trailing: [u8; 32]) -> [u8; 36] {
    let mut response = [0_u8; 36];
    response[..4].copy_from_slice(&[
        BENCHMARK_VERSION,
        summary.rollover_inputs,
        summary.transactions,
        summary.signature_jobs,
    ]);
    response[4..].copy_from_slice(&trailing);
    response
}

fn benchmark_failure_reply(error: BenchmarkError) -> Reply {
    Reply(match error {
        BenchmarkError::Graph(_) => 0x6f10,
        BenchmarkError::Crypto => 0x6f11,
        BenchmarkError::SignatureCreation => 0x6f13,
        BenchmarkError::PublicKeyMismatch => 0x6f15,
        BenchmarkError::SignatureCountMismatch => 0x6f16,
    })
}

fn next_expected_command<'a>(
    comm: &'a mut Comm,
    expected: BenchmarkInstruction,
) -> Option<Command<'a>> {
    let command = comm.next_command();
    match command.decode::<BenchmarkInstruction>() {
        Ok(instruction) if instruction == expected => Some(command),
        Ok(_) => {
            let _ = command.reply(&[], StatusWords::BadIns);
            None
        }
        Err(reply) => {
            let _ = command.reply(&[], reply);
            None
        }
    }
}

fn review_benchmark_policy(comm: &mut Comm) -> bool {
    let fields = [
        Field {
            name: "Vault balance",
            value: "2.1 BTC",
        },
        Field {
            name: "Monthly allowance",
            value: "0.1 BTC",
        },
        Field {
            name: "Emergency access",
            value: "0.5 BTC",
        },
        Field {
            name: "Emergency access delay",
            value: "1 week",
        },
    ];
    let tag_values = TagValueList::new(&fields, 2, false, false);
    let approval = InfoLongPress::new(
        "Approve annual vault policy",
        Some(&APP_ICON),
        "Hold to approve",
        TuneIndex::Success,
    );
    NbglGenericReview::new()
        .add_content(NbglPageContent::TagValueList(tag_values))
        .add_content(NbglPageContent::InfoLongPress(approval))
        .show(comm, "Reject")
}

#[derive(Debug, Clone, Copy)]
struct BenchmarkTimings {
    key_derivation_ms: u32,
    graph_ms: u32,
    signing_ms: u32,
}

fn show_benchmark_complete(comm: &mut Comm, signatures: u8, timings: Option<BenchmarkTimings>) {
    let message = match timings {
        Some(timings) => {
            let total_ms = timings
                .key_derivation_ms
                .saturating_add(timings.graph_ms)
                .saturating_add(timings.signing_ms);
            format!(
                "Benchmark complete\n\nKey derivation: {} ms\nGraph: {} ms\n{signatures} signatures: {} ms\nFull workload: {total_ms} ms",
                timings.key_derivation_ms, timings.graph_ms, timings.signing_ms
            )
        }
        None => format!(
            "Benchmark complete\n\n{signatures} signatures created\n\nConnect the benchmark runner for phase timings."
        ),
    };
    let _ = NbglAction::new()
        .glyph(&APP_ICON)
        .message(&message)
        .action_text("Return home")
        .show(comm);
}

#[unsafe(no_mangle)]
extern "C" fn sample_main() {
    let comm = init_comm(&COMM);
    let _ = (APP_NAME, APP_TAGLINE);
    loop {
        show_home(comm);
    }
}
