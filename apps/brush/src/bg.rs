use clap::Parser;
use std::io::Write;

use brush_core::{ExecutionResult, builtins};

/// Moves a job to run in the background and prints its job line.
#[derive(Parser)]
pub struct BgCommand {
    job_specs: Vec<String>,
}

impl builtins::Command for BgCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<brush_core::ExecutionResult, Self::Error> {
        let mut exit_code = ExecutionResult::success();
        let specs: Vec<Option<&String>> = if self.job_specs.is_empty() { vec![None] } else { self.job_specs.iter().map(Some).collect() };
        for spec in specs {
            let job = match spec { Some(s) => context.shell.jobs_mut().resolve_job_spec(s), None => context.shell.jobs_mut().current_job_mut() };
            if let Some(job) = job {
                job.move_to_background()?;
                let line = format!("[{}]{} {} &", job.id, job.annotation(), job.command_line);
                writeln!(context.stdout(), "{line}")?;
            } else {
                match spec {
                    Some(s) => writeln!(context.stderr(), "{}: {}: no such job", context.command_name, s)?,
                    None => writeln!(context.stderr(), "{}: no current job", context.command_name)?,
                }
                exit_code = ExecutionResult::general_error();
            }
        }
        Ok(exit_code)
    }
}
