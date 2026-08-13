#![no_std]
#![no_main]

extern crate alloc;

mod benchmark;

use alloc::format;
use anzen_cold_signer::PROTOCOL_NAME;
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
const RUN_INS: u8 = 0x21;
const COMPLETE_INS: u8 = 0x22;
const BENCHMARK_VERSION: u8 = 2;

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
    Run,
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
            (RUN_INS, 0, 0) => Ok(Self::Run),
            (COMPLETE_INS, 0, 0) => Ok(Self::Complete),
            (PREPARE_INS | RUN_INS | COMPLETE_INS, _, _) => Err(StatusWords::BadP1P2),
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
    let mut context = match BenchmarkContext::prepare(12) {
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
    spinner.show("Signing vault policy");
    if let Err(error) = context.run() {
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
    let mut context = match BenchmarkContext::prepare(rollover_inputs) {
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
    let mut prepared_response = [0_u8; 36];
    prepared_response[..4].copy_from_slice(&[
        BENCHMARK_VERSION,
        summary.rollover_inputs,
        summary.transactions,
        summary.signature_jobs,
    ]);
    prepared_response[4..].copy_from_slice(&public_key);
    if comm.send(&prepared_response, StatusWords::Ok).is_err() {
        return;
    }

    spinner.show("Ready for timed signing");
    let run_command = match next_expected_command(comm, BenchmarkInstruction::Run) {
        Some(command) => command,
        None => return,
    };
    if !run_command.get_data().is_empty() {
        let _ = run_command.reply(&[], StatusWords::BadLen);
        return;
    }
    spinner.show("Signing vault policy");
    let transcript = match context.run() {
        Ok(transcript) => transcript,
        Err(error) => {
            let _ = run_command.reply(&[], benchmark_failure_reply(error));
            return;
        }
    };
    let mut run_response = [0_u8; 36];
    run_response[..4].copy_from_slice(&[
        BENCHMARK_VERSION,
        summary.rollover_inputs,
        summary.transactions,
        summary.signature_jobs,
    ]);
    run_response[4..].copy_from_slice(&transcript);
    let comm = match run_command
        .into_response()
        .extend(&run_response)
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
    let signing_milliseconds = match complete_command.get_data().try_into() {
        Ok(encoded) => u32::from_be_bytes(encoded),
        Err(_) => {
            let _ = complete_command.reply(&[], StatusWords::BadLen);
            return;
        }
    };
    if complete_command
        .reply(&[BENCHMARK_VERSION], StatusWords::Ok)
        .is_err()
    {
        return;
    }
    show_benchmark_complete(comm, summary.signature_jobs, Some(signing_milliseconds));
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

fn show_benchmark_complete(comm: &mut Comm, signatures: u8, signing_milliseconds: Option<u32>) {
    let message = match signing_milliseconds {
        Some(milliseconds) => {
            let seconds = milliseconds / 1_000;
            let decimal = (milliseconds % 1_000) / 100;
            let per_signature = milliseconds / u32::from(signatures);
            format!(
                "Vault policy signed\n\n{signatures} signatures created\nSigning time  {seconds}.{decimal} s\n{per_signature} ms per signature"
            )
        }
        None => format!(
            "Vault policy signed\n\n{signatures} signatures created\n\nConnect the benchmark runner to measure signing time."
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
