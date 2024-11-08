//! Module providing analysis and reporting of network check results.
//!
//! # Analysis Features
//!
//! This module analyzes data from the [Store] to provide:
//! - Outage detection and tracking
//! - Success/failure statistics per check type
//! - Latency analysis
//! - Report generation
//!
//! The main entry point is the [analyze] function which generates
//! a comprehensive report of the store's contents.
//!
//! # Examples
//!
//! ```rust,no_run
//! use netpulse::{store::Store, analyze};
//!
//! let store = Store::load().unwrap();
//! let report = analyze::analyze(&store).unwrap();
//! println!("{}", report);
//! ```
//!
//! # Report Sections
//!
//! The analysis report contains several sections:
//! - General statistics (total checks, success rates)
//! - HTTP-specific metrics
//! - Outage analysis
//! - Store metadata (hashes, versions)

use crate::errors::AnalysisError;
use crate::records::{Check, CheckFlag, CheckType};
use crate::store::Store;

use std::fmt::{Display, Write};
use std::hash::Hash;

/// Represents a period of consecutive failed checks.
///
/// An outage is defined by:
/// - A start check that failed
/// - An optional end check (None if outage is ongoing)
/// - All failed checks during the outage period
///
/// This struct helps track and analyze network connectivity issues
/// over time.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct Outage<'check> {
    /// First check that failed, marking the start of the outage
    start: &'check Check,
    /// Last failed check before connectivity was restored
    /// [None] if the outage is still ongoing
    end: Option<&'check Check>,
    /// All checks that failed during this outage period
    all: Vec<&'check Check>,
}

impl<'check> Outage<'check> {
    /// Creates a new outage from its constituent checks.
    ///
    /// # Arguments
    ///
    /// * `start` - The first failed check
    /// * `end` - Optional last failed check (None if ongoing)
    /// * `all_checks` - Slice of all failed checks in this period
    pub(crate) fn new(
        start: &'check Check,
        end: Option<&'check Check>,
        all_checks: &[&'check Check],
    ) -> Self {
        Self {
            start,
            end,
            all: all_checks.to_vec(),
        }
    }
}

impl Display for Outage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.end.is_some() {
            writeln!(
                f,
                "From {} To {}",
                humantime::format_rfc3339_seconds(self.start.timestamp_parsed()),
                humantime::format_rfc3339_seconds(self.end.unwrap().timestamp_parsed())
            )?;
        } else {
            writeln!(
                f,
                "From {} STILL ONGOING",
                humantime::format_rfc3339_seconds(self.start.timestamp_parsed()),
            )?;
        }
        writeln!(f, "Checks: {}", self.all.len())?;
        writeln!(
            f,
            "Type: {}",
            self.start.calc_type().unwrap_or(CheckType::Unknown)
        )?;
        Ok(())
    }
}

/// Display a formatted list of checks.
///
/// Each check is formatted with:
/// - Index number
/// - Indented check details
/// - Nested line breaks preserved
///
/// # Arguments
///
/// * `group` - Slice of check references to format
/// * `f` - String buffer to write formatted output
///
/// # Errors
///
/// Returns [AnalysisError] if string formatting fails.
pub fn display_group(group: &[&Check], f: &mut String) -> Result<(), std::fmt::Error> {
    if group.is_empty() {
        writeln!(f, "\t<Empty>")?;
        return Ok(());
    }
    for (cidx, check) in group.iter().enumerate() {
        writeln!(f, "{cidx}:")?;
        writeln!(f, "\t{}", check.to_string().replace("\n", "\n\t"))?;
    }
    Ok(())
}

/// Generate a comprehensive analysis report for the given store.
///
/// The report includes:
/// - General check statistics
/// - HTTP-specific metrics
/// - Outage analysis
/// - Store metadata
///
/// # Errors
///
/// Returns [AnalysisError] if:
/// - Report string formatting fails
/// - Store hash calculation fails
///
/// # Example
///
/// ```rust,no_run
/// use netpulse::{store::Store, analyze};
///
/// let store = Store::load().unwrap();
/// let report = analyze::analyze(&store).unwrap();
/// println!("{}", report);
/// ```
pub fn analyze(store: &Store) -> Result<String, AnalysisError> {
    let mut f = String::new();
    barrier(&mut f, "General")?;
    generalized(store, &mut f)?;
    barrier(&mut f, "HTTP")?;
    generic_type_analyze(store, &mut f, CheckType::Http)?;
    barrier(&mut f, "ICMPv4")?;
    generic_type_analyze(store, &mut f, CheckType::IcmpV4)?;
    barrier(&mut f, "ICMPv6")?;
    generic_type_analyze(store, &mut f, CheckType::IcmpV6)?;
    barrier(&mut f, "IPv4")?;
    gereric_ip_analyze(store, &mut f, CheckFlag::IPv4)?;
    barrier(&mut f, "IPv6")?;
    gereric_ip_analyze(store, &mut f, CheckFlag::IPv6)?;
    barrier(&mut f, "Outages")?;
    outages(store, &mut f)?;
    barrier(&mut f, "Store Metadata")?;
    store_meta(store, &mut f)?;

    Ok(f)
}

/// Adds a section divider to the report with a title.
///
/// Creates a divider line of '=' characters with the title centered.
///
/// # Errors
///
/// Returns [AnalysisError] if string formatting fails.
fn barrier(f: &mut String, title: &str) -> Result<(), AnalysisError> {
    writeln!(f, "{:=<10}{:=<90}", "", format!(" {title} "))?;
    Ok(())
}

/// Writes a key-value pair to the report in aligned columns.
///
/// Format: `<key>: <value>`
fn key_value_write(
    f: &mut String,
    title: &str,
    content: impl Display,
) -> Result<(), std::fmt::Error> {
    writeln!(f, "{:<20}: {:<78}", title, content.to_string())
}

/// Analyzes and formats outage information from the store.
///
/// Groups consecutive failed checks by check type and creates
/// Outage records for reporting.
fn outages(store: &Store, f: &mut String) -> Result<(), AnalysisError> {
    let all_checks: Vec<&Check> = store.checks().iter().collect();
    let mut outages: Vec<Outage> = Vec::new();
    let fails_exist = all_checks
        .iter()
        .fold(true, |fails_exist, c| fails_exist & !c.is_success());
    if !fails_exist || all_checks.is_empty() {
        writeln!(f, "None\n")?;
        return Ok(());
    }

    for check_type in CheckType::all() {
        let checks: Vec<&&Check> = all_checks
            .iter()
            .filter(|c| c.calc_type().unwrap_or(CheckType::Unknown) == *check_type)
            .collect();

        let fail_groups = fail_groups(&checks);
        for group in fail_groups {
            // writeln!(f, "Group {gidx}:")?;
            // display_group(group, f)?;
            if !group.is_empty() {
                outages.push(Outage::new(
                    checks.first().unwrap(),
                    Some(checks.last().unwrap()),
                    &group,
                ));
            }
        }
    }

    for outage in outages {
        writeln!(f, "{outage}")?;
    }
    Ok(())
}

/// Find groups of consecutive failed checks.
///
/// Groups are formed when:
/// - Checks are consecutive by index
/// - All checks in group are failures
/// - Gap between groups is > 1 check
fn fail_groups<'check>(checks: &[&&'check Check]) -> Vec<Vec<&'check Check>> {
    let failed_idxs: Vec<usize> = checks
        .iter()
        .enumerate()
        .filter(|(_idx, c)| !c.is_success())
        .map(|(idx, _c)| idx)
        .collect();
    if failed_idxs.is_empty() {
        return Vec::new();
    }
    let mut groups: Vec<Vec<&Check>> = Vec::new();

    let mut first = failed_idxs[0];
    let mut last = first;
    for idx in failed_idxs {
        if idx == last + 1 {
            last = idx;
        } else {
            let mut group: Vec<&Check> = Vec::new();
            for check in checks.iter().take(last + 1).skip(first) {
                group.push(*check);
            }
            groups.push(group);

            first = idx;
        }
    }

    groups
}

/// Analyze metrics for a specific check type.
///
/// Calculates and formats:
/// - Total check count
/// - Success/failure counts
/// - Success ratio
/// - First/last check timestamps
///
/// # Errors
///
/// Returns [AnalysisError] if formatting fails.
fn analyze_check_type_set(
    f: &mut String,
    all: &[&Check],
    successes: &[&Check],
) -> Result<(), AnalysisError> {
    if all.is_empty() {
        writeln!(f, "None\n")?;
        return Ok(());
    }
    key_value_write(f, "checks", format!("{:08}", all.len()))?;
    key_value_write(f, "checks ok", format!("{:08}", successes.len()))?;
    key_value_write(
        f,
        "checks bad",
        format!("{:08}", all.len() - successes.len()),
    )?;
    key_value_write(
        f,
        "success ratio",
        format!(
            "{:03.02}%",
            success_ratio(all.len(), successes.len()) * 100.0
        ),
    )?;
    key_value_write(
        f,
        "first check at",
        humantime::format_rfc3339_seconds(all.first().unwrap().timestamp_parsed()),
    )?;
    key_value_write(
        f,
        "last check at",
        humantime::format_rfc3339_seconds(all.last().unwrap().timestamp_parsed()),
    )?;
    writeln!(f)?;
    Ok(())
}

/// Write general check statistics section of the report.
///
/// Includes metrics across all check types combined.
fn generalized(store: &Store, f: &mut String) -> Result<(), AnalysisError> {
    if store.checks().is_empty() {
        writeln!(f, "Store has no checks yet\n")?;
        return Ok(());
    }
    let all: Vec<&Check> = store.checks().iter().collect();
    let successes: Vec<&Check> = store.checks().iter().filter(|c| c.is_success()).collect();
    analyze_check_type_set(f, &all, &successes)?;
    Ok(())
}

/// Write check statistics section of the report for `check_type`.
///
/// Analyzes and formats statistics for IPv4/IPv6 checks.
///
/// Collects all checks that used that IP and generates a statistical report including:
/// - Total number of that IP checks
/// - Success/failure counts
/// - Success ratio
/// - First/last check timestamps
///
/// Checks with ambiguous or invalid IP flags are excluded and logged as errors.
///
/// # Errors
///
/// Returns [AnalysisError] if:
/// - Report formatting fails
/// - Check type analysis fails
///
/// # Warning Messages
///
/// Prints warning to stderr if:
/// - Check has both IPv4 and IPv6 flags set
/// - Check has no IP version flags set
fn gereric_ip_analyze(
    store: &Store,
    f: &mut String,
    ip_check_flag: CheckFlag,
) -> Result<(), AnalysisError> {
    if ![CheckFlag::IPv4, CheckFlag::IPv6].contains(&ip_check_flag) {
        panic!("check flag is not IPv4 or IPv6: {ip_check_flag:?}");
    }
    let all: Vec<&Check> = store
        .checks()
        .iter()
        .filter(|c| match c.ip_type() {
            Ok(ip) => ip,
            Err(err) => {
                eprintln!("check '{}' has bad flags: {err}", c.get_hash());
                return false;
            }
        } == CheckFlag::IPv4
        )
        .collect();
    let successes: Vec<&Check> = all.clone().into_iter().filter(|c| c.is_success()).collect();
    analyze_check_type_set(f, &all, &successes)?;
    Ok(())
}
/// Includes metrics across all check types combined.
fn generic_type_analyze(
    store: &Store,
    f: &mut String,
    check_type: CheckType,
) -> Result<(), AnalysisError> {
    let all: Vec<&Check> = store
        .checks()
        .iter()
        .filter(|c| c.calc_type().unwrap_or(CheckType::Unknown) == check_type)
        .collect();
    let successes: Vec<&Check> = all.clone().into_iter().filter(|c| c.is_success()).collect();
    analyze_check_type_set(f, &all, &successes)?;
    Ok(())
}

/// Write store metadata section of the report.
///
/// Includes:
/// - Hash of in-memory data structure
/// - Hash of store file on disk
fn store_meta(store: &Store, f: &mut String) -> Result<(), AnalysisError> {
    key_value_write(f, "Hash Datastructure", store.display_hash())?;
    key_value_write(f, "Hash Store File", store.display_hash_of_file()?)?;
    // TODO: write version of store in file and in memory
    Ok(())
}

/// Calculate the success ratio of a subset compared to total.
///
/// Returns value between 0.0 and 1.0.
#[inline]
fn success_ratio(all_checks: usize, subset: usize) -> f64 {
    subset as f64 / all_checks as f64
}
