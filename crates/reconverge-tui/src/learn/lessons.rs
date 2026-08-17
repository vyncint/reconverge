//! The four lessons, embedded whole: prose from `docs/learn/` (pages
//! separated by `---` lines), kernels as source snippets, replays as
//! the shipped fixture witnesses. Everything is compiled in — learn mode
//! runs with no network, no analysis step, and no files on disk, which
//! the flow tests prove by running it in an empty directory.
//!
//! TODO(phase-r): like the explain pages, `include_str!` reaches outside
//! the package directory; mirror into OUT_DIR before publishing.

use reconverge_artifacts::witness::WitnessArtifact;

use crate::load::nfc;

pub struct Lesson {
    pub id: &'static str,
    title: &'static str,
    pub pages: Vec<Page>,
}

pub struct Page {
    body: String,
    /// A kernel snippet shown under the prose.
    pub code: Option<&'static str>,
    /// A replay driven by the debugger machinery, shown under the code.
    pub witness: Option<WitnessArtifact>,
}

impl Lesson {
    #[must_use]
    pub fn title(&self) -> &str {
        self.title
    }
}

impl Page {
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }
}

// The crucial lines only: at 80x24 a witness page shows prose, excerpt,
// and the live replay together, so excerpts stay under five lines (the
// full kernels live in the lint samples and the explain pages).
const DIVERGENT_KERNEL: &str = "\
let i = thread::index_1d();
if i.get() % 2 == 0 {
    thread::sync_threads(); // <- RC001
}";

const COLLECTIVE_KERNEL: &str = "\
if i.get() % 2 == 0 {
    vote = warp::ballot_sync(0xffff_ffff, true); // <- RC002
}";

const RECONVERGED_KERNEL: &str = "\
if i.get() % 2 == 0 {
    // per-lane work, no barrier in here
}
// the paths rejoin at the branch's post-dominator
thread::sync_threads(); // every lane arrives";

const RC001_WITNESS: &str =
    include_str!("../../../../fixtures/witness/rc001-divergent-barrier.json");
const RC002_WITNESS: &str = include_str!("../../../../fixtures/witness/rc002-partial-mask.json");
const CLEAN_WITNESS: &str = include_str!("../../../../fixtures/witness/reconverged-clean.json");

/// Split a lesson file into pages on `---` separator lines and zip with
/// per-page extras. Page counts are locked by the unit tests below, so a
/// drifted edit of `docs/learn/` fails the build's tests, not the reader.
fn pages(
    prose: &'static str,
    extras: &[(Option<&'static str>, Option<&'static str>)],
) -> Vec<Page> {
    let bodies: Vec<String> = prose
        .split("\n---\n")
        .map(|page| nfc(page.trim_matches('\n')))
        .collect();
    assert_eq!(bodies.len(), extras.len(), "page metadata out of step");
    bodies
        .into_iter()
        .zip(extras)
        .map(|(body, (code, witness))| Page {
            body,
            code: *code,
            witness: witness.map(|json| {
                let mut artifact: WitnessArtifact =
                    serde_json::from_str(json).expect("embedded witness is valid witness.v1");
                // The same display normalization the debugger applies.
                artifact.kernel = nfc(&artifact.kernel);
                artifact.verdict.message = nfc(&artifact.verdict.message);
                artifact
            }),
        })
        .collect()
}

/// The four lessons, in teaching order.
#[must_use]
pub fn lessons() -> Vec<Lesson> {
    vec![
        Lesson {
            id: "divergence",
            title: "divergence — how a warp splits",
            pages: pages(
                include_str!("../../../../docs/learn/divergence.md"),
                &[
                    (None, None),
                    (Some(DIVERGENT_KERNEL), Some(RC001_WITNESS)),
                    (None, None),
                ],
            ),
        },
        Lesson {
            id: "barriers",
            title: "barriers — why a divergent sync hangs",
            pages: pages(
                include_str!("../../../../docs/learn/barriers.md"),
                &[
                    (None, None),
                    (Some(DIVERGENT_KERNEL), Some(RC001_WITNESS)),
                    (None, None),
                ],
            ),
        },
        Lesson {
            id: "masks",
            title: "masks — who joins a warp collective",
            pages: pages(
                include_str!("../../../../docs/learn/masks.md"),
                &[
                    (None, None),
                    (Some(COLLECTIVE_KERNEL), Some(RC002_WITNESS)),
                    (None, None),
                ],
            ),
        },
        Lesson {
            id: "reconvergence",
            title: "reconvergence — the fix",
            pages: pages(
                include_str!("../../../../docs/learn/reconvergence.md"),
                &[
                    (None, None),
                    (Some(RECONVERGED_KERNEL), Some(CLEAN_WITNESS)),
                    (None, None),
                ],
            ),
        },
    ]
}

#[cfg(test)]
mod tests {
    use reconverge_artifacts::witness::VerdictKind;

    use super::*;

    #[test]
    fn four_lessons_of_three_pages_each() {
        let all = lessons();
        assert_eq!(all.len(), 4);
        assert_eq!(
            all.iter().map(|l| l.id).collect::<Vec<_>>(),
            ["divergence", "barriers", "masks", "reconvergence"]
        );
        for lesson in &all {
            assert_eq!(lesson.pages.len(), 3, "{}", lesson.id);
            for (i, page) in lesson.pages.iter().enumerate() {
                let body = page.body();
                assert!(!body.is_empty(), "{} page {i}", lesson.id);
                // Warps are never described as running in lockstep.
                assert!(
                    !body.to_ascii_lowercase().contains("lockstep"),
                    "{} page {i} says lockstep",
                    lesson.id
                );
            }
            // The middle page is the interactive one: kernel + replay.
            assert!(lesson.pages[1].code.is_some(), "{}", lesson.id);
            assert!(lesson.pages[1].witness.is_some(), "{}", lesson.id);
        }
    }

    #[test]
    fn embedded_witnesses_are_valid_and_the_fix_completes() {
        let all = lessons();
        let verdict = |lesson: &Lesson| lesson.pages[1].witness.as_ref().unwrap().verdict.kind;
        assert_eq!(verdict(&all[0]), VerdictKind::UndefinedBehavior);
        assert_eq!(verdict(&all[1]), VerdictKind::UndefinedBehavior);
        assert_eq!(verdict(&all[2]), VerdictKind::UndefinedBehavior);
        assert_eq!(
            verdict(&all[3]),
            VerdictKind::Completed,
            "the reconvergence lesson teaches the shape that cannot hang"
        );
        for lesson in &all {
            let witness = lesson.pages[1].witness.as_ref().unwrap();
            assert_eq!(witness.lanes, 32);
            assert_eq!(witness.initial_lane_states.len(), 32);
        }
    }

    #[test]
    fn witness_pages_fit_the_80x24_layout_budget() {
        // Inner height at 80x24 is 22: header + blank (2), prose, blank,
        // excerpt, replay panel (8). Prose + excerpt must stay within the
        // remainder in both languages or the replay clips off screen.
        for lesson in lessons() {
            for (i, page) in lesson.pages.iter().enumerate() {
                if page.witness.is_none() {
                    continue;
                }
                let code_lines = page.code.map_or(0, |c| c.lines().count());
                let body_lines = page.body().lines().count();
                assert!(
                    body_lines + code_lines <= 11,
                    "{} page {i}: {body_lines} prose + {code_lines} code lines \
                     overflow the 80x24 budget",
                    lesson.id
                );
            }
        }
    }

    #[test]
    fn every_lesson_speaks_of_its_own_subject() {
        let all = lessons();
        assert!(all[0].pages[0].body().contains("SIMT"));
        assert!(all[1].pages[0].body().contains("sync_threads"));
        assert!(all[2].pages[0].body().contains("participation mask"));
        assert!(all[3].pages[0].body().contains("post-dominator"));
    }
}
