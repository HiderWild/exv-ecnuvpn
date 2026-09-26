//! 独立服务代理二进制入口。

use std::process::ExitCode;

use exv_vpn_darwin_service_agent::{ServiceAgentOutcome, cli::run_process};

fn main() -> ExitCode {
    match run_process() {
        Ok(None | Some(ServiceAgentOutcome::Accepted)) => ExitCode::SUCCESS,
        Ok(Some(ServiceAgentOutcome::Rejected(error))) => {
            eprintln!("{}", error.code());
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("{}", error.code());
            ExitCode::FAILURE
        }
    }
}
