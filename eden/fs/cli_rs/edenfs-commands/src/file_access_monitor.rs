/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fmt::Debug;
use std::fs::File as FsFile;
use std::io::BufRead;
use std::io::BufReader;
use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use clap::Parser;
use edenfs_client::EdenFsInstance;
use edenfs_utils::path_from_bytes;
use hg_util::path::expand_path;
use serde::Deserialize;
use serde::Serialize;

use crate::ExitCode;
use crate::Subcommand;

// This path should be the same as the path defined in
// EdenServiceHandler.cpp::semifuture_startFileAccessMonitor
// Change this with caution since FAM is running privileged.
const TMP_FAM_OUTPUT_DIR_PATH: &str = "/tmp/edenfs/fam/";

#[cfg(target_os = "macos")]
#[derive(Parser, Debug)]
#[clap(
    name = "file-access-monitor",
    alias = "fam",
    about = "File Access Monitor(FAM) to audit processes.\nAvailable only on macOS."
)]
pub struct FileAccessMonitorCmd {
    #[clap(subcommand)]
    subcommand: FileAccessMonitorSubcommand,
}

#[derive(Parser, Debug)]
#[clap(about = "Start File Access Monitor. File access events are logged to the output file.")]
struct StartCmd {
    #[clap(
        help = "A list of paths that FAM should use as filters when monitoring file access events.",
        short = 'p',
        long = "paths",
        required = true
    )]
    paths: Vec<String>,

    #[clap(
        help = "The path of the output file where the file access events are logged.",
        short = 'o',
        long = "output"
    )]
    output: Option<String>,

    #[clap(
        help = "When set, the command returns immediately, leaving FAM running in the background.\nTo stop it, run 'eden fam stop'.\nThis is required since Ctrl-C is not killing FAM and timeout is not supported for now.",
        short = 'b',
        long = "background",
        required = true
    )]
    background: bool,

    #[clap(
        help = "How long FAM should run in seconds. This should not be set when '--background' is set.",
        short = 't',
        long = "timeout",
        default_value = "30",
        conflicts_with = "background"
    )]
    timeout: u64,

    #[clap(help = "When set, the output file is uploaded and a link is returned.")]
    upload: bool,
}

#[async_trait]
impl crate::Subcommand for StartCmd {
    async fn run(&self) -> Result<ExitCode> {
        // Check the temporary folder exists, otherwise create it
        let tmp_dir_path = PathBuf::from(TMP_FAM_OUTPUT_DIR_PATH);
        if !tmp_dir_path.exists() {
            std::fs::create_dir_all(tmp_dir_path)?;
        }

        println!("Starting File Access Monitor");

        let mut monitor_paths: Vec<PathBuf> = Vec::new();

        for path in &self.paths {
            monitor_paths.push(expand_path(path));
        }

        let output_path = self.output.as_ref().map(expand_path);

        let start_result = EdenFsInstance::global()
            .start_file_access_monitor(&monitor_paths, output_path, self.upload)
            .await?;

        println!("File Access Monitor started [pid {}]", start_result.pid);
        println!(
            "Temp output file path: {}",
            path_from_bytes(&start_result.tmpOutputPath)?.display()
        );

        if self.background {
            println!(
                "File Access Monitor is running in the background.\nTo stop, run 'eden fam stop'."
            );
            return Ok(0);
        }

        // TODO[lxw]: handle timeout

        stop_fam().await
    }
}

async fn stop_fam() -> Result<ExitCode> {
    let stop_result = EdenFsInstance::global().stop_file_access_monitor().await?;
    println!("File Access Monitor stopped");
    // TODO: handle the case when the output file is specified
    let output_path = path_from_bytes(&stop_result.specifiedOutputPath)?;

    println!("Output file saved to {}", output_path.display());

    if stop_result.shouldUpload {
        // TODO[lxw]: handle uploading outputfile
        println!("Upload not implemented yet");
        return Ok(1);
    }
    Ok(0)
}

#[derive(Parser, Debug)]
#[clap(about = "Stop File Access Monitor to audit processes.")]
struct StopCmd {}

#[async_trait]
impl crate::Subcommand for StopCmd {
    async fn run(&self) -> Result<ExitCode> {
        stop_fam().await
    }
}

#[derive(Parser, Debug)]
#[clap(about = "Read the output file and parse it to a summary of file access events.")]
struct ReadCmd {
    #[clap(
        help = "Path to the FAM output file. This file is generated by FAM when monitoring file system activity.",
        short = 'p',
        long = "path",
        required = true
    )]
    path: String,

    #[clap(
        help = "Print verbose information about parsed events.",
        long = "verbose",
        required = false
    )]
    verbose: bool,

    #[clap(
        help = "Specify the maximum number of PIDs to be displayed in the output. If set to 0, all PIDs will be displayed.",
        short = 'k',
        required = false,
        default_value = "10"
    )]
    count: usize,
}

#[derive(Serialize, Deserialize, Debug)]
struct FileItem {
    path: String,
    truncated: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct File {
    source: Option<FileItem>,
    target: Option<FileItem>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Process {
    pid: u64,
    ppid: u64,
    uid: u64,
    ancestors: Vec<u64>,
    args: Vec<String>,
    command: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Event {
    event_type: String,
    file: File,
    process: Process,
    event_timestamp: u64,
}

fn parse_events<R: BufRead>(reader: R) -> Result<Vec<Event>> {
    let mut objects: Vec<Event> = Vec::new();
    let mut new_object = String::new();
    for line in reader.lines().map_while(Result::ok) {
        new_object.push_str(&line);
        if line == "}" {
            objects.push(serde_json::from_str(&new_object)?);
            new_object.clear();
        }
    }
    Ok(objects)
}

fn sort_pids(events: &[Event]) -> Vec<(u64, u64, u64)> {
    // Count the number of events with the same PID
    let mut pid_counts: HashMap<u64, (u64, u64)> = HashMap::new(); // pid -> (counter, ppid)
    for event in events {
        let process = &event.process;
        let count = pid_counts.entry(process.pid).or_insert((0, process.ppid));
        count.0 += 1;
    }

    // Sort the results so we can find the top k
    let mut sorted_pids: Vec<(u64, u64, u64)> = pid_counts
        .into_iter()
        .map(|(pid, (count, ppid))| (pid, count, ppid))
        .collect();
    sorted_pids.sort_by_key(|(_, count, _)| Reverse(*count));
    sorted_pids
}

#[async_trait]
impl crate::Subcommand for ReadCmd {
    async fn run(&self) -> Result<ExitCode> {
        // construct the path
        let path = PathBuf::from(&self.path);
        let file = FsFile::open(path)?;
        let reader = BufReader::new(file);

        let events = parse_events(reader)?;

        if self.verbose {
            println!("Parsed {} objects", events.len());
            println!("{:#?}", events);
        }

        let sorted_pids = sort_pids(&events);

        let slice = if self.count == 0 {
            &sorted_pids
        } else {
            &sorted_pids[..self.count.min(sorted_pids.len())]
        };

        // Print the top results
        println!("{:<6} | {:<7} | {}", "PID", "PPID", "Counts");
        for (pid, count, ppid) in slice {
            println!("{:<6} | {:<7} | {}", pid, ppid, count);
        }
        Ok(0)
    }
}

#[derive(Parser, Debug)]
enum FileAccessMonitorSubcommand {
    Start(StartCmd),
    Stop(StopCmd),
    Read(ReadCmd),
}

#[async_trait]
impl Subcommand for FileAccessMonitorCmd {
    async fn run(&self) -> Result<ExitCode> {
        use FileAccessMonitorSubcommand::*;
        let sc: &(dyn Subcommand + Send + Sync) = match &self.subcommand {
            Start(cmd) => cmd,
            Stop(cmd) => cmd,
            Read(cmd) => cmd,
        };
        sc.run().await
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn test_parse_complete_event() {
        let event = r#"
{
  "event_type": "NOTIFY_OPEN",
  "file": {
    "target": {
      "path": "/tmp/test_dir/test_file_open",
      "truncated": false
    }
  },
  "process": {
    "ancestors": [],
    "args": [],
    "command": "/usr/local/bin/python3",
    "pid": 22222,
    "ppid": 99999,
    "uid": 67890
  },
  "event_timestamp": 1740024705
}
        "#;
        let parsed = parse_events(BufReader::new(Cursor::new(event)));
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap().len(), 1);
    }

    #[test]
    fn test_parse_complete_events() {
        let event = r#"
{
  "event_type": "NOTIFY_OPEN",
  "file": {
    "target": {
      "path": "/tmp/test_dir/test_file_open",
      "truncated": false
    }
  },
  "process": {
    "ancestors": [],
    "args": [],
    "command": "/usr/local/bin/python3",
    "pid": 22222,
    "ppid": 99999,
    "uid": 67890
  },
  "event_timestamp": 1740024705
}

{
  "event_type": "NOTIFY_OPEN",
  "file": {
    "target": {
      "path": "/tmp/test_dir/test_file_open",
      "truncated": false
    }
  },
  "process": {
    "ancestors": [],
    "args": [],
    "command": "/usr/local/bin/python3",
    "pid": 22222,
    "ppid": 99999,
    "uid": 67890
  },
  "event_timestamp": 1740024705
}
        "#;
        let parsed = parse_events(BufReader::new(Cursor::new(event)));
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap().len(), 2);
    }

    #[test]
    fn test_parse_incomplete_events() {
        let event = r#"
{
  "event_type": "NOTIFY_OPEN",
  "file": {
    "target": {
      "path": "/tmp/test_dir/test_file_open",
      "truncated": false
    }
  },
  "process": {
    "ancestors": [],
    "args": [],
    "command": "/usr/local/bin/python3",
    "pid": 22222,
    "ppid": 99999,
    "uid": 67890
  },
  "event_timestamp": 1740024705
}

{
  "event_type": "NOTIFY_OPEN",
  "file": {
    "target": {
      "path": "/tmp/test_dir/test_file_open",
      "truncated": false
    }
  },
  "process": {
    "ancestors": [],
    "args": [],
    "command": "/usr/local/bin/pyth
        "#;
        let parsed = parse_events(BufReader::new(Cursor::new(event)));
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap().len(), 1);
    }

    #[test]
    fn test_sort_pids() {
        fn make_event(pid: u64, ppid: u64) -> Event {
            Event {
                event_type: "NOTIFY_OPEN".to_string(),
                file: File {
                    source: None,
                    target: Some(FileItem {
                        path: "what".to_string(),
                        truncated: false,
                    }),
                },
                process: Process {
                    ancestors: vec![],
                    args: vec![],
                    command: "what".to_string(),
                    pid,
                    ppid,
                    uid: 67890,
                },
                event_timestamp: 1740024705,
            }
        }

        let events = vec![
            make_event(66778, 22309),
            make_event(980066, 11759),
            make_event(1, 2),
            make_event(1, 2),
            make_event(980066, 11759),
            make_event(980066, 11759),
            make_event(66778, 22309),
            make_event(980066, 11759),
            make_event(1, 2),
            make_event(980066, 11759),
            make_event(1, 2),
        ];

        let sorted_pids = sort_pids(&events);
        assert_eq!(sorted_pids.len(), 3);
        assert_eq!(sorted_pids[0].0, 980066);
        assert_eq!(sorted_pids[1].0, 1);
        assert_eq!(sorted_pids[2].0, 66778);
    }
}
