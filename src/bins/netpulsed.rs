//! Daemon control binary for starting/stopping the netpulse daemon.
//!
//! This binary manages the netpulse daemon process:
//! - Starting the daemon with proper privileges
//! - Stopping running daemon instances
//! - Checking daemon status
//!
//! # Usage
//!
//! Use the `--help` flag for more information about the usage.
//!
//! # Privileges
//!
//! The daemon requires root to start but drops privileges to run as the netpulse user.
//! Note that ICMP checks require `CAP_NET_RAW` capability which is lost on privilege drop.
//!
//! # Files
//!
//! - PID file: `/var/run/netpulse/netpulsed.pid`
//! - Info log: `/var/log/netpulse/info.log`
//! - Error log: `/var/log/netpulse/error.log`

use core::panic;
use std::fs::{self, File};
use std::path::PathBuf;

use daemonize::Daemonize;
use getopts::Options;
use netpulse::store::Store;
use netpulse::{DAEMON_LOG_ERR, DAEMON_LOG_INF, DAEMON_PID_FILE, DAEMON_USER};
use nix::errno::Errno;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;

mod daemon;
use daemon::daemon;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let program = &args[0];
    let mut opts = Options::new();
    opts.optflag("h", "help", "print this help menu");
    opts.optflag("V", "version", "print the version");
    opts.optflag("s", "start", "start the netpulse daemon");
    opts.optflag(
        "d",
        "daemon",
        "run directly as the daemon, do not setup a pidfile or drop privileges",
    );
    #[cfg(debug_assertions)]
    opts.optflag("", "fail", "add a failed http check");
    opts.optflag("i", "info", "info about the running netpulse daemon");
    opts.optflag("e", "end", "stop the running netpulse daemon");
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => {
            panic!("{}", f.to_string())
        }
    };

    if matches.opt_present("help") {
        print_usage(program, opts);
    } else if matches.opt_present("version") {
        println!("{} {}", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"))
    } else if matches.opt_present("start") {
        startd();
    } else if matches.opt_present("info") {
        infod();
    } else if matches.opt_present("end") {
        endd();
    } else if matches.opt_present("fail") {
        #[cfg(debug_assertions)]
        fail();
    } else if matches.opt_present("test") {
        daemon();
    } else {
        print_usage(program, opts);
    }
}

#[cfg(debug_assertions)]
fn fail() {
    use std::str::FromStr;

    use netpulse::records::{Check, CheckFlag};

    let mut store = Store::load().expect("could not load store");
    let time = std::time::SystemTime::now();
    store.add_check(Check::new(
        time,
        CheckFlag::IPv4 | CheckFlag::TypeHTTP,
        None,
        std::net::IpAddr::from_str("0.0.0.0").unwrap(),
    ));
    store.save().expect("could not safe store")
}

fn getpid() -> Option<i32> {
    if !fs::exists(DAEMON_PID_FILE).expect("couldn't check if the pid file exists") {
        None
    } else {
        let pid_raw = fs::read_to_string(DAEMON_PID_FILE)
            .expect("pid file does not exist")
            .trim()
            .to_string();
        let pid = match pid_raw.parse() {
            Ok(pid) => pid,
            Err(err) => {
                eprintln!("Error while parsing the pid from file ('{pid_raw}'): {err}");
                return None;
            }
        };
        Some(pid)
    }
}

fn infod() {
    match getpid() {
        Some(pid) => {
            if pid_runs(pid) {
                println!("netpulsed is running with pid {pid}")
            } else {
                println!(
                    "the pid file exists with pid {pid}, but no process with that pid is running"
                )
            }
        }
        None => println!("netpulsed is not running"),
    }
}

fn pid_runs(pid: i32) -> bool {
    fs::exists(format!("/proc/{pid}")).expect("could not check if the process exists")
}

fn endd() {
    root_guard();
    let mut terminated = false;
    let pid: Pid = match getpid() {
        None => {
            println!("netpulsed is not running");
            return;
        }
        Some(raw) => Pid::from_raw(raw),
    };

    match signal::kill(pid, Signal::SIGTERM) {
        Ok(()) => {
            println!("Sent termination signal to netpulsed (pid: {pid})");
            // Optionally: wait to confirm process ended
            // Could check /proc/{pid} or try kill(0)
        }
        Err(e) => {
            match e {
                // no such process
                Errno::ESRCH => {
                    terminated = true;
                }
                _ => {
                    eprintln!("Failed to terminate netpulsed: {e}");
                    std::process::exit(1)
                }
            }
        }
    }

    let sent_sig = std::time::Instant::now();
    while !terminated && sent_sig.elapsed().as_secs() < 5 {
        if pid_runs(pid.as_raw()) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        } else {
            terminated = true
        }
    }
    if !terminated {
        println!("netpulsed (pid {pid}) is taking too long to terminate, killing it",);
        match signal::kill(pid, Signal::SIGKILL) {
            Ok(()) => {
                println!("Sent kill signal to netpulsed (pid: {pid})");
            }
            Err(e) => {
                eprintln!("Failed to kill netpulsed: {e}");
            }
        }
    }
    if fs::exists(DAEMON_PID_FILE).expect("could not check if the pid file exists") {
        eprintln!("The pid file ({DAEMON_PID_FILE}) still exists even though the daemon is not running, removing it");
        if let Err(err) = fs::remove_file(DAEMON_PID_FILE) {
            eprintln!("Could not remove the pid file: {err}")
        }
    }
}

fn print_usage(program: &str, opts: Options) {
    let brief = format!("Usage: {} [options]", program);
    print!("{}", opts.usage(&brief));
}

fn root_guard() {
    if !nix::unistd::getuid().is_root() {
        eprintln!("This needs to be run as root");
        std::process::exit(1)
    }
}

fn startd() {
    root_guard();
    let path = Store::path();
    let parent_path = path.parent().expect("store file has no parent directory");
    println!("Parent: {parent_path:?}");

    let pid_path = PathBuf::from(DAEMON_PID_FILE);
    let pid_parent_path = pid_path.parent().expect("pid file has no parent directory");
    println!("Pid Parent: {pid_parent_path:?}");

    let logfile = File::create(DAEMON_LOG_INF).expect("could not open info logfile");
    let errfile = File::create(DAEMON_LOG_ERR).expect("could not open error logfile");

    let user = nix::unistd::User::from_name(DAEMON_USER)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        .expect("could not get user for netpulse")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "netpulse user not found"))
        .expect("could not get user for netpulse");

    fs::create_dir_all(parent_path).expect("could not create the store directory");
    fs::create_dir_all(pid_parent_path).expect("could not create the pid directory");
    std::os::unix::fs::chown(
        pid_parent_path,
        Some(user.uid.into()),
        Some(user.gid.into()),
    )
    .expect("could not set permissions for the netpulse run directory");

    // NOTE: Daemonize is the defacto standard way of becoming a daemon in rust (besides extra
    // tools like systemd or writing it all yourself with nix or just the libc).
    // Sadly, Daemonize just drops all capabilities when we become a daemon, including an important
    // one: CAP_NET_RAW. This capability allows us to use raw sockets, which are required for
    // things like ICMP (ping) messages.
    // I have implemented ICMP checks, but the daemon drops the CAP_NET_RAW capability and then is
    // no longer allowed to make the custom pings.
    let daemonize = Daemonize::new()
        .pid_file(pid_path)
        .chown_pid_file(true)
        .working_directory(parent_path)
        .user("netpulse")
        .group("netpulse")
        .stdout(logfile)
        .stderr(errfile)
        .umask(0o022); // rw-r--r--

    println!("daemon setup done");

    let outcome = daemonize.execute();
    match outcome {
        daemonize::Outcome::Parent(result) => {
            if result.is_ok() {
                println!("netpulsed was started",);
            } else {
                eprintln!("error while starting netpulsed: {}", result.unwrap_err());
            }
        }
        daemonize::Outcome::Child(result) => {
            if result.is_ok() {
                daemon();
            } else {
                panic!("error while starting the daemon: {}", result.unwrap_err())
            }
        }
    }
}
