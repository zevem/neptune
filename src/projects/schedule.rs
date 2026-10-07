//! When a watch that runs on a schedule is due, as sums over times that are
//! handed in: nothing here reads a clock. A schedule never runs early. What
//! it missed while Neptune was closed is made up for once, a moment after
//! Neptune opened, and never run by run.
use std::time::{SystemTime, UNIX_EPOCH};

/// The shortest time between two runs of a schedule,
pub const MIN_MINUTES: u32 = 15;
/// and the longest: a week.
pub const MAX_MINUTES: u32 = 7 * 24 * 60;
/// What was missed while Neptune was closed runs this long after it opened,
pub const START_DELAY: u64 = 30;
/// and each further one this much later, so they do not all begin at once.
pub const STAGGER: u64 = 5;

/// `time` in seconds since the Unix epoch.
pub fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn step(minutes: u32) -> u64 {
    u64::from(minutes.clamp(MIN_MINUTES, MAX_MINUTES)) * 60
}

/// The first run of a schedule that is made, allowed or resumed at `now`.
pub fn first(minutes: u32, now: SystemTime) -> u64 {
    seconds(now).saturating_add(step(minutes))
}

/// The run that follows the one that was due at `due`, seen at `now`: the
/// next time on the schedule's own beat that is still to come. Runs whose
/// time passed meanwhile are left out, not caught up.
pub fn after(minutes: u32, due: u64, now: SystemTime) -> u64 {
    let (step, now) = (step(minutes), seconds(now));
    if due > now {
        return due;
    }
    due.saturating_add(((now - due) / step + 1).saturating_mul(step))
}

/// How many runs of a schedule whose next run was due at `next` have passed
/// by `now`. None while it is not due.
pub fn missed(minutes: u32, next: u64, now: SystemTime) -> Option<u64> {
    let now = seconds(now);
    (next <= now).then(|| (now - next) / step(minutes) + 1)
}

/// One schedule that was due while Neptune was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Late {
    pub id: u64,
    /// The runs it missed, this one among them.
    pub runs: u64,
    /// When it runs, once.
    pub at: u64,
}

/// What Neptune makes up for as it opens. `schedules` are those that may
/// run, as `(id, minutes, next)`; `opened` is when Neptune started, `now`
/// when their project was read back, and `ahead` how many late runs of
/// other projects are placed already. Each late schedule runs once.
pub fn late(
    schedules: impl IntoIterator<Item = (u64, u32, u64)>,
    opened: SystemTime,
    now: SystemTime,
    ahead: u64,
) -> Vec<Late> {
    let begin = seconds(opened)
        .saturating_add(START_DELAY)
        .max(seconds(now));
    schedules
        .into_iter()
        .filter_map(|(id, minutes, next)| Some((id, missed(minutes, next, now)?)))
        .zip(ahead..)
        .map(|((id, runs), place)| Late {
            id,
            runs,
            at: begin.saturating_add(place.saturating_mul(STAGGER)),
        })
        .collect()
}

/// "14:05 UTC": the time of day of `at`. Times are said in UTC, as the
/// project's decisions are: the local offset is the system's to know.
pub fn clock(at: u64) -> String {
    let of_day = at % (24 * 3600);
    format!("{:02}:{:02} UTC", of_day / 3600, of_day % 3600 / 60)
}

/// "every 30 min", "every 2 h", "every 1 h 30 min".
pub fn every(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("every {minutes} min"),
        (hours, 0) => format!("every {hours} h"),
        (hours, minutes) => format!("every {hours} h {minutes} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }
    const HOUR: u64 = 3600;

    #[test]
    fn a_schedule_runs_on_its_own_beat_and_never_early() {
        // Made at ten o'clock, an hourly schedule first runs at eleven.
        assert_eq!(first(60, at(10 * HOUR)), 11 * HOUR);
        // No schedule runs more often than every quarter of an hour, or
        // less often than weekly, whatever it was saved with.
        assert_eq!(first(1, at(0)), 15 * 60);
        assert_eq!(first(u32::MAX, at(0)), 7 * 24 * HOUR);
        // Seen on time, or a little late, the next run keeps the beat.
        assert_eq!(after(60, 11 * HOUR, at(11 * HOUR)), 12 * HOUR);
        assert_eq!(after(60, 11 * HOUR, at(11 * HOUR + 59)), 12 * HOUR);
        // Seen hours late, the runs in between are left out: the next one
        // is still to come, and on the beat.
        assert_eq!(after(60, 11 * HOUR, at(14 * HOUR + 600)), 15 * HOUR);
        assert_eq!(after(60, 11 * HOUR, at(14 * HOUR)), 15 * HOUR);
        // A run that is not due yet stays where it is.
        assert_eq!(after(60, 11 * HOUR, at(10 * HOUR)), 11 * HOUR);
        for now in [0, 1, 899, 900, 901, 86_399, 86_400, 1_000_000] {
            let next = after(15, 900, at(now));
            assert!(next > now || next == 900, "{now}");
            assert_eq!(next % 900, 0);
        }
    }

    #[test]
    fn what_was_missed_while_neptune_was_closed_runs_once_a_moment_after_it_opens() {
        assert_eq!(missed(60, 11 * HOUR, at(11 * HOUR - 1)), None);
        assert_eq!(missed(60, 11 * HOUR, at(11 * HOUR)), Some(1));
        assert_eq!(missed(60, 11 * HOUR, at(12 * HOUR - 1)), Some(1));
        assert_eq!(missed(60, 11 * HOUR, at(14 * HOUR + 5)), Some(4));

        // Opened at 20:00 after a night closed: the hourly one missed nine
        // runs, the one every quarter of an hour far more, and a third is
        // not due. Each late one runs once, half a minute after opening and
        // five seconds apart.
        let opened = at(20 * HOUR);
        let schedules = [
            (3, 60, 11 * HOUR),
            (4, 240, 21 * HOUR),
            (7, 15, 19 * HOUR + 50 * 60),
        ];
        assert_eq!(
            late(schedules, opened, at(20 * HOUR + 2), 0),
            [
                Late {
                    id: 3,
                    runs: 10,
                    at: 20 * HOUR + 30
                },
                Late {
                    id: 7,
                    runs: 1,
                    at: 20 * HOUR + 35
                },
            ]
        );
        // Another project's late runs come after those placed already.
        assert_eq!(
            late([(1, 60, 0)], opened, at(20 * HOUR + 2), 2)[0].at,
            20 * HOUR + 40
        );
        // A project read back long after Neptune opened is never early
        // either: its late runs begin when it was read.
        assert_eq!(
            late([(1, 60, 0)], opened, at(22 * HOUR), 0)[0].at,
            22 * HOUR
        );
        assert!(late([(1, 60, 21 * HOUR)], opened, opened, 0).is_empty());
    }

    #[test]
    fn times_and_beats_are_said_in_words() {
        assert_eq!(clock(0), "00:00 UTC");
        assert_eq!(clock(3 * 24 * HOUR + 9 * HOUR + 5 * 60 + 59), "09:05 UTC");
        assert_eq!(every(15), "every 15 min");
        assert_eq!(every(120), "every 2 h");
        assert_eq!(every(90), "every 1 h 30 min");
        assert_eq!(seconds(UNIX_EPOCH - Duration::from_secs(5)), 0);
    }
}
