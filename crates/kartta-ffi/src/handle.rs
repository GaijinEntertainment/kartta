use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};

use kartta::{BuildError, CancelToken, Index, Language, Stopped};
use serde::Serialize;

use crate::ABI_VERSION;
use crate::built::{self, BROKEN, Built, Held, Shared, StoppedBuild};
use crate::error::{Error, ErrorCode};
use crate::query::{FILE_QUERIES, QUERIES};
use crate::wire::files::{GrepParams, GrepView};
use crate::wire::{
    self, BuildParams, By, CancelAnswer, CancelParams, NoParams, OpenParams, RefreshParams,
    StatsAnswer, StoppedView,
};

/// One tree, and the engine its last finished build made of it: what a C handle points to.
///
/// [`Kartta::open`] only names the tree. The `build` method walks it and builds the file index and every graph in one
/// pass, and every other method that queries asks the engine that build left. Queries run concurrently with each other
/// and with `cancel`; builds run one at a time, and a build drops what the handle held before it walks, so the
/// handle never holds two engines and a query during the walk answers the code `building`. A query that was already
/// running keeps the engine it asked until it returns. A build that fails leaves the handle without an engine. A build
/// a cancel stops leaves no engine, and the index of what it reached when that holds an entry, which answers `glob`,
/// `list`, `fuzzy`, `grep` and `stats`, each saying so.
///
/// The `refresh` method brings the engine up to the tree in place and holds it alone while it runs, so a query waits
/// for a refresh under way, and refreshes run one at a time. A build cancels the refreshes in flight as it drops the
/// engine they refresh, and does not wait for them: a cancel stops a refresh only before its walk ends, so one past it
/// keeps the dropped engine until it answers, as a running query does.
///
/// A build that asks to `watch` leaves a [`kartta::Follower`] refreshing the engine from each change the operating
/// system reports, as a refresh does, under a token of its own that `cancel` never reaches. The next build cancels it
/// before it empties the slot, and so does dropping the handle; the follow then ends at its next look.
///
/// The locks are taken in one order: the refreshes' registry, then the builds', then the slot, then the engine's own.
#[derive(Debug)]
pub struct Kartta {
    root: PathBuf,
    held: RwLock<Option<Held>>,
    builds: Mutex<Calls>,
    greps: Mutex<Calls>,
    refreshes: Mutex<Calls>,
    one_build_at_a_time: Mutex<()>,
}

const CANCELLED_AHEAD_LIMIT: usize = 1024;

#[derive(Debug, Default)]
struct Calls {
    entered: u64,
    in_flight: Vec<InFlight>,
    cancelled_ahead: Vec<u64>,
    binding_ids_cancelled_ahead: Vec<u64>,
}

#[derive(Debug)]
struct InFlight {
    ticket: u64,
    id: Option<u64>,
    binding_id: Option<u64>,
    token: CancelToken,
}

impl InFlight {
    const fn named(&self, by: By) -> Option<u64> {
        match by {
            By::Id => self.id,
            By::BindingId => self.binding_id,
        }
    }
}

#[derive(Debug, Serialize)]
struct About {
    abi: u32,
    version: &'static str,
    methods: Vec<&'static str>,
    languages: &'static [&'static str],
}

impl Kartta {
    /// Opens a handle on the tree that `params`, a JSON object `{"root": <path>}`, names. Nothing is walked until a
    /// build.
    ///
    /// # Errors
    ///
    /// The code `invalid_request` or `invalid_params` when `params` is not that object, and `invalid_root` when the
    /// root is not a directory this process can read the metadata of.
    pub fn open(params: &[u8]) -> Result<Self, Error> {
        let OpenParams { root } = wire::params(params)?;
        let invalid_root = |reason: &dyn std::fmt::Display| {
            Error::new(
                ErrorCode::InvalidRoot,
                format!("{}: {reason}", root.display()),
            )
        };
        match fs::metadata(&root) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(invalid_root(&"not a directory")),
            Err(error) => return Err(invalid_root(&error)),
        }
        Ok(Self {
            root,
            held: RwLock::new(None),
            builds: Mutex::default(),
            greps: Mutex::default(),
            refreshes: Mutex::default(),
            one_build_at_a_time: Mutex::new(()),
        })
    }

    /// Answers `method` with `params`, a JSON object or nothing, as the JSON `{"ok": <answer>}` or
    /// `{"err": {"code": <code>, "message": <message>}}`. `WIRE.md` beside the crate's manifest states the methods and
    /// their params; an unknown method answers the code `unknown_method`, listing the known ones.
    ///
    /// A panic answers the code `internal` and never unwinds out of the call, so a caller on a thread of its own or
    /// behind a C boundary needs no guard, and the handle stays usable.
    #[must_use]
    pub fn call(&self, method: &str, params: &[u8]) -> Vec<u8> {
        panic::catch_unwind(AssertUnwindSafe(|| self.answer(method, params)))
            .unwrap_or_else(|payload| Err(Error::panicked(payload.as_ref())))
            .unwrap_or_else(|error| wire::err(&error))
    }

    fn answer(&self, method: &str, params: &[u8]) -> Result<Vec<u8>, Error> {
        match method {
            "build" => self.build(&wire::params(params)?),
            "cancel" => wire::ok(&self.cancel(&wire::params(params)?)),
            "grep" => self.grep(&wire::params(params)?),
            "refresh" => self.refresh(&wire::params(params)?),
            "stats" => {
                let NoParams {} = wire::params(params)?;
                match self.held()? {
                    Held::Built(shared) => {
                        let built = read(&shared)?;
                        wire::ok(&StatsAnswer::new(
                            &built.engine,
                            &built.errors,
                            built.watch.as_ref(),
                        ))
                    }
                    Held::Stopped(stopped) => wire::ok(&StatsAnswer::stopped(&stopped)),
                }
            }
            "version" => {
                let NoParams {} = wire::params(params)?;
                wire::ok(&About {
                    abi: ABI_VERSION,
                    version: env!("CARGO_PKG_VERSION"),
                    methods: methods(),
                    languages: kartta_languages::NAMES,
                })
            }
            _ => {
                if let Some(&(_, query)) = FILE_QUERIES.iter().find(|(name, _)| *name == method) {
                    return self.on_index(|index, stopped| query(index, stopped, params));
                }
                let Some(&(_, query)) = QUERIES.iter().find(|(name, _)| *name == method) else {
                    return Err(Error::new(
                        ErrorCode::UnknownMethod,
                        format!(
                            "no method {method:?}; the methods are {}",
                            methods().join(", ")
                        ),
                    ));
                };
                let built = self.built()?;
                query(&*read(&built)?, params)
            }
        }
    }

    fn build(&self, params: &BuildParams) -> Result<Vec<u8>, Error> {
        self.build_languages(params, params.languages()?)
    }

    fn build_languages(
        &self,
        params: &BuildParams,
        languages: Vec<Box<dyn Language>>,
    ) -> Result<Vec<u8>, Error> {
        let options = params.options()?;

        let (ticket, token) = lock(&self.builds).enter(params.id, params.binding_id);
        let _one_build = lock(&self.one_build_at_a_time);
        let mut refreshes = lock(&self.refreshes);
        refreshes.cancel(None);
        let mut slot = self.held.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(Held::Built(replaced)) = &*slot {
            replaced.unfollow();
        }
        *slot = None;
        drop(slot);
        drop(refreshes);
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            if params.watch {
                Shared::build_watched(&self.root, &options, languages, &token)
            } else {
                Shared::build(&self.root, &options, languages, &token).map(|shared| (shared, None))
            }
        }));

        let mut builds = lock(&self.builds);
        builds.leave(ticket);
        let (shared, follower) = match outcome {
            Ok(Err(BuildError::Stopped(stopped))) => Err(self.keep(*stopped)),
            Ok(Ok(built)) => Ok(built),
            Ok(Err(error)) => Err(Error::new(ErrorCode::BuildFailed, error.to_string())),
            Err(payload) => Err(Error::panicked(payload.as_ref())),
        }?;
        let shared = Arc::new(shared);
        if let Some(follower) = follower {
            drop(follower.follow(&shared));
        }
        let answer = {
            let built = read(&shared)?;
            wire::ok(&StatsAnswer::new(
                &built.engine,
                &built.errors,
                built.watch.as_ref(),
            ))
        };
        self.hold(Held::Built(shared));
        drop(builds);
        if token.is_cancelled() {
            return Err(Error::new(
                ErrorCode::BuildCancelled,
                "the cancel came after the build finished; every query answers from it",
            ));
        }
        answer
    }

    /// Holds what a cancel left of a build, unless its walk reached nothing, and answers the failure the build ends
    /// with.
    fn keep(&self, stopped: Stopped) -> Error {
        if stopped.index.is_empty() {
            return Error::new(
                ErrorCode::BuildCancelled,
                "a cancel stopped the build before its walk reached an entry; call build",
            );
        }
        let stopped = StoppedBuild::new(stopped);
        let message = format!("a cancel stopped the build; {}", stopped.answers());
        self.hold(Held::Stopped(Arc::new(stopped)));
        Error::new(ErrorCode::BuildCancelled, message)
    }

    fn hold(&self, held: Held) {
        *self.held.write().unwrap_or_else(PoisonError::into_inner) = Some(held);
    }

    fn grep(&self, params: &GrepParams) -> Result<Vec<u8>, Error> {
        self.on_index(|index, stopped| {
            let (ticket, token) = lock(&self.greps).enter(params.id, params.binding_id);
            let searched = panic::catch_unwind(AssertUnwindSafe(|| params.search(index, &token)));
            lock(&self.greps).leave(ticket);

            let matches =
                searched.unwrap_or_else(|payload| Err(Error::panicked(payload.as_ref())))?;
            wire::ok(&GrepView::new(index, &matches, stopped))
        })
    }

    /// What `ask` answers from the index the handle holds, told when that index is a stopped build's.
    fn on_index(
        &self,
        ask: impl FnOnce(&Index, Option<StoppedView>) -> Result<Vec<u8>, Error>,
    ) -> Result<Vec<u8>, Error> {
        match self.held()? {
            Held::Built(shared) => {
                let built = read(&shared)?;
                ask(built.engine.index(), None)
            }
            Held::Stopped(stopped) => ask(&stopped.index, Some(stopped.view())),
        }
    }

    fn refresh(&self, params: &RefreshParams) -> Result<Vec<u8>, Error> {
        let paths = params.paths.as_deref();
        built::misspelled(paths)?;
        let (built, (ticket, token)) = {
            let mut refreshes = lock(&self.refreshes);
            (self.built()?, refreshes.enter(params.id, params.binding_id))
        };
        let refreshed = panic::catch_unwind(AssertUnwindSafe(|| {
            built
                .built
                .write()
                .map_err(|_| broken())?
                .refresh(paths, &token)
        }));
        lock(&self.refreshes).leave(ticket);

        refreshed.unwrap_or_else(|payload| Err(Error::panicked(payload.as_ref())))
    }

    fn cancel(&self, params: &CancelParams) -> CancelAnswer {
        let every = params.build.is_none() && params.grep.is_none() && params.refresh.is_none();
        let named = |id: Option<u64>| id.map(|id| (params.by, id));
        let builds =
            (every || params.build.is_some()) && lock(&self.builds).cancel(named(params.build));
        let greps =
            (every || params.grep.is_some()) && lock(&self.greps).cancel(named(params.grep));
        let refreshes = (every || params.refresh.is_some())
            && lock(&self.refreshes).cancel(named(params.refresh));

        CancelAnswer {
            stopped: builds || greps || refreshes,
        }
    }

    /// The engine to answer from, or what names the fix when the handle holds no engine.
    fn built(&self) -> Result<Arc<Shared>, Error> {
        match self.held()? {
            Held::Built(shared) => Ok(shared),
            Held::Stopped(stopped) => Err(Error::new(
                ErrorCode::NotBuilt,
                format!(
                    "a cancel stopped the last build, so this handle holds no graph; {}",
                    stopped.answers()
                ),
            )),
        }
    }

    fn held(&self) -> Result<Held, Error> {
        if let Some(held) = self.finished() {
            return Ok(held);
        }
        let idle = {
            let builds = lock(&self.builds);
            if let Some(held) = self.finished() {
                return Ok(held);
            }
            builds.in_flight.is_empty()
        };
        if idle {
            Err(Error::new(
                ErrorCode::NotBuilt,
                "no build has finished on this handle; call build first",
            ))
        } else {
            Err(Error::new(
                ErrorCode::Building,
                "a build is running; ask again once it finishes",
            ))
        }
    }

    fn finished(&self) -> Option<Held> {
        self.held
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Ends the follow of the engine the handle holds, which a query or a refresh still running may outlive the handle
/// with.
impl Drop for Kartta {
    fn drop(&mut self) {
        if let Some(Held::Built(shared)) =
            &*self.held.get_mut().unwrap_or_else(PoisonError::into_inner)
        {
            shared.unfollow();
        }
    }
}

/// The engine to answer from, or the error that names the fix once a call panicked while it changed the engine.
fn read(shared: &Shared) -> Result<RwLockReadGuard<'_, Built>, Error> {
    shared.built.read().map_err(|_| broken())
}

fn broken() -> Error {
    Error::new(ErrorCode::Internal, BROKEN)
}

fn methods() -> Vec<&'static str> {
    let mut methods: Vec<&'static str> = ["build", "cancel", "grep", "refresh", "stats", "version"]
        .into_iter()
        .chain(QUERIES.iter().map(|&(name, _)| name))
        .chain(FILE_QUERIES.iter().map(|&(name, _)| name))
        .collect();
    methods.sort_unstable();
    methods
}

impl Calls {
    fn enter(&mut self, id: Option<u64>, binding_id: Option<u64>) -> (u64, CancelToken) {
        let token = CancelToken::new();
        for (by, named) in [(By::Id, id), (By::BindingId, binding_id)] {
            let remembered = self.remembered(by);
            if let Some(named) = named
                && let Some(at) = remembered.iter().position(|&ahead| ahead == named)
            {
                remembered.remove(at);
                token.cancel();
            }
        }
        self.entered += 1;
        self.in_flight.push(InFlight {
            ticket: self.entered,
            id,
            binding_id,
            token: token.clone(),
        });
        (self.entered, token)
    }

    fn leave(&mut self, ticket: u64) {
        self.in_flight.retain(|build| build.ticket != ticket);
    }

    fn cancel(&mut self, named: Option<(By, u64)>) -> bool {
        let mut stopped = false;
        for build in &self.in_flight {
            if named.is_none_or(|(by, id)| build.named(by) == Some(id)) {
                build.token.cancel();
                stopped = true;
            }
        }
        let Some((by, id)) = named else {
            return stopped;
        };
        let remembered = self.remembered(by);
        if !stopped && !remembered.contains(&id) {
            if remembered.len() == CANCELLED_AHEAD_LIMIT {
                remembered.remove(0);
            }
            remembered.push(id);
        }
        stopped
    }

    const fn remembered(&mut self, by: By) -> &mut Vec<u64> {
        match by {
            By::Id => &mut self.cancelled_ahead,
            By::BindingId => &mut self.binding_ids_cancelled_ahead,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    use kartta::{Extraction, Followed, Language, RawExtraction, Repository, Resolver};
    use serde_json::Value;

    use super::{
        BuildParams, By, CANCELLED_AHEAD_LIMIT, Calls, CancelParams, Kartta, Shared, lock, wire,
    };

    fn handle() -> (tempfile::TempDir, Kartta) {
        let tree = tempfile::tempdir().expect("a temporary directory");
        let params = serde_json::json!({ "root": tree.path() }).to_string();
        let kartta = Kartta::open(params.as_bytes()).expect("the tree opens");
        (tree, kartta)
    }

    fn built() -> (tempfile::TempDir, Kartta) {
        let (tree, kartta) = handle();
        let built: Value = serde_json::from_slice(&kartta.call("build", b"")).expect("JSON");
        assert!(built.get("ok").is_some(), "{built}");
        (tree, kartta)
    }

    #[test]
    fn a_query_waits_while_the_engine_is_written() {
        let (tree, kartta) = built();
        let engine = kartta.built().expect("an engine");
        let mut written = engine.built.write().expect("the engine is not poisoned");
        let asking = Barrier::new(2);

        thread::scope(|scope| {
            let stats = scope.spawn(|| {
                asking.wait();
                kartta.call("stats", b"")
            });
            asking.wait();
            thread::sleep(Duration::from_millis(50));
            assert!(!stats.is_finished(), "stats answered past the write");
            fs::write(tree.path().join("main.go"), "package main\n").expect("main.go");
            written.engine.refresh(None).expect("the refresh");
            drop(written);

            let answer: Value =
                serde_json::from_slice(&stats.join().expect("stats returns")).expect("JSON");
            assert_eq!(
                answer["ok"]["entries"], 1,
                "stats answers the engine the write left: {answer}"
            );
        });
    }

    #[test]
    fn an_engine_a_panic_left_half_written_answers_internal_naming_build() {
        let (_tree, kartta) = built();
        let engine = kartta.built().expect("an engine");
        let panicked = panic::catch_unwind(AssertUnwindSafe(|| {
            let _written = engine.built.write().expect("the engine is not poisoned");
            panic!("halfway");
        }));
        assert!(panicked.is_err());

        let answer: Value = serde_json::from_slice(&kartta.call("stats", b"")).expect("JSON");

        assert_eq!(answer["err"]["code"], "internal", "{answer}");
        let message = answer["err"]["message"].as_str().expect("a message");
        assert!(message.contains("call build"), "{message}");
    }

    #[test]
    fn a_cancel_naming_no_call_stops_every_build_grep_and_refresh_in_flight() {
        let (_tree, kartta) = handle();
        let (_, build) = lock(&kartta.builds).enter(Some(1), None);
        let (_, grep) = lock(&kartta.greps).enter(Some(2), None);
        let (_, refresh) = lock(&kartta.refreshes).enter(Some(3), None);

        let answer = kartta.cancel(&CancelParams::default());

        assert!(answer.stopped);
        assert!(build.is_cancelled() && grep.is_cancelled() && refresh.is_cancelled());
    }

    #[test]
    fn a_build_cancels_the_refreshes_in_flight_as_it_drops_their_engine() {
        let (_tree, kartta) = built();
        let (_, refresh) = lock(&kartta.refreshes).enter(Some(1), None);

        let rebuilt: Value = serde_json::from_slice(&kartta.call("build", b"")).expect("JSON");

        assert!(rebuilt.get("ok").is_some(), "{rebuilt}");
        assert!(refresh.is_cancelled());
    }

    /// The engine of a build of `kartta` that watches its tree.
    fn watched(kartta: &Kartta) -> Arc<Shared> {
        let built: Value =
            serde_json::from_slice(&kartta.call("build", br#"{"watch": true}"#)).expect("JSON");
        assert_eq!(built["ok"]["watch"]["following"], true, "{built}");
        kartta.built().expect("an engine")
    }

    #[test]
    fn a_cancel_naming_no_call_leaves_the_follow_running() {
        let (_tree, kartta) = handle();
        let engine = watched(&kartta);

        let cancelled = kartta.call("cancel", b"{}");

        assert_eq!(cancelled, br#"{"ok":{"stopped":false}}"#);
        assert!(!engine.token().is_cancelled());
    }

    #[test]
    fn a_build_ends_the_follow_of_the_engine_it_replaces() {
        let (_tree, kartta) = handle();
        let replaced = watched(&kartta);

        let rebuilt = kartta.call("build", b"");

        assert!(rebuilt.starts_with(br#"{"ok":"#));
        assert!(replaced.token().is_cancelled());
    }

    #[test]
    fn dropping_the_handle_ends_the_follow_of_its_engine() {
        let (_tree, kartta) = handle();
        let engine = watched(&kartta);

        drop(kartta);

        assert!(engine.token().is_cancelled());
    }

    #[test]
    fn a_cancel_naming_a_grep_leaves_a_build_of_that_id_running() {
        let (_tree, kartta) = handle();
        let (_, build) = lock(&kartta.builds).enter(Some(2), None);
        let (_, grep) = lock(&kartta.greps).enter(Some(2), None);

        let answer = kartta.cancel(&CancelParams {
            grep: Some(2),
            ..CancelParams::default()
        });

        assert!(answer.stopped);
        assert!(grep.is_cancelled() && !build.is_cancelled());
    }

    #[test]
    fn a_cancel_repeated_before_its_build_enters_is_remembered_once() {
        let mut builds = Calls::default();

        builds.cancel(Some((By::Id, 7)));
        builds.cancel(Some((By::Id, 7)));

        assert_eq!(builds.cancelled_ahead, [7]);
    }

    #[test]
    fn a_cancel_naming_a_call_in_flight_trips_its_token_and_remembers_nothing() {
        let mut calls = Calls::default();
        let (_, token) = calls.enter(Some(3), None);

        assert!(calls.cancel(Some((By::Id, 3))), "the call was in flight");
        assert!(token.is_cancelled());
        assert_eq!(calls.cancelled_ahead, Vec::<u64>::new());
    }

    #[test]
    fn a_call_in_flight_stops_by_either_of_its_names_and_not_by_its_id_under_the_other() {
        let mut calls = Calls::default();
        let (_, token) = calls.enter(Some(3), Some(9));

        assert!(
            !calls.cancel(Some((By::Id, 9))),
            "9 is the call's binding id"
        );
        assert!(
            !calls.cancel(Some((By::BindingId, 3))),
            "3 is the call's id"
        );
        assert!(!token.is_cancelled());
        assert!(calls.cancel(Some((By::BindingId, 9))));
        assert!(token.is_cancelled());
    }

    #[test]
    fn stale_cancels_past_the_limit_forget_the_oldest_and_a_later_cancel_still_stops_its_call() {
        let mut calls = Calls::default();
        let last_id = u64::try_from(CANCELLED_AHEAD_LIMIT).expect("the limit fits a u64") + 1;

        for id in 1..=last_id {
            calls.cancel(Some((By::Id, id)));
        }

        assert_eq!(calls.cancelled_ahead.len(), CANCELLED_AHEAD_LIMIT);
        let (_, first) = calls.enter(Some(1), None);
        let (_, last) = calls.enter(Some(last_id), None);
        assert!(!first.is_cancelled(), "the oldest id was forgotten");
        assert!(last.is_cancelled(), "the latest id was remembered");
    }

    #[test]
    fn stale_cancels_by_id_past_the_limit_leave_a_binding_id_remembered() {
        let mut calls = Calls::default();
        let last_id = u64::try_from(CANCELLED_AHEAD_LIMIT).expect("the limit fits a u64") + 1;
        calls.cancel(Some((By::BindingId, 1)));

        for id in 1..=last_id {
            calls.cancel(Some((By::Id, id)));
        }

        let (_, bound) = calls.enter(None, Some(1));
        assert!(bound.is_cancelled(), "the binding id was remembered apart");
    }

    #[derive(Debug, Clone, Copy)]
    enum CancelAt {
        FirstFileRead,
        ResolverBuilt,
        Resolving,
        ResolverPanics,
    }

    /// A language that cancels the build of id 1 on `.0` once the build comes to `.1`.
    #[derive(Debug)]
    struct Cancelling(Arc<Kartta>, CancelAt);

    fn cancel_the_build(kartta: &Kartta) {
        kartta.cancel(&CancelParams {
            build: Some(1),
            ..CancelParams::default()
        });
    }

    struct Resolving(Option<Arc<Kartta>>);

    impl Resolver for Resolving {
        fn resolve(
            &self,
            raw: RawExtraction,
            path: &str,
            _repository: &dyn Repository,
        ) -> Extraction {
            if let Some(kartta) = &self.0 {
                cancel_the_build(kartta);
            }
            raw.into_extraction(path).0
        }
    }

    impl Language for Cancelling {
        fn name(&self) -> &'static str {
            "cancelling"
        }

        fn claims(&self, path: &str) -> bool {
            path.rsplit('.').next() == Some("defs")
        }

        fn fingerprint(&self) -> &[u8] {
            b"cancelling 1"
        }

        fn extract(&self, _source: &[u8]) -> RawExtraction {
            if matches!(self.1, CancelAt::FirstFileRead) {
                cancel_the_build(&self.0);
            }
            RawExtraction::default()
        }

        fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
            assert!(
                !matches!(self.1, CancelAt::ResolverPanics),
                "a bad resolver"
            );
            if matches!(self.1, CancelAt::ResolverBuilt) {
                cancel_the_build(&self.0);
            }
            let cancelling = matches!(self.1, CancelAt::Resolving);
            Box::new(Resolving(cancelling.then(|| Arc::clone(&self.0))))
        }
    }

    /// A handle on a tree of `a.defs`, `b.defs` and `c/d.defs`, after a one-threaded build of id 1 that cancels itself
    /// at `at`, and the build's answer.
    fn cancelled_at(at: CancelAt) -> (tempfile::TempDir, Arc<Kartta>, Value) {
        let (tree, kartta) = handle();
        for (path, content) in [
            ("a.defs", "needle\n"),
            ("b.defs", "b\n"),
            ("c/d.defs", "d\n"),
        ] {
            let path = tree.path().join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
            fs::write(path, content).expect("the file");
        }
        let kartta = Arc::new(kartta);
        let params: BuildParams =
            wire::params(br#"{"id": 1, "threads": 1}"#).expect("the params read");

        let built = kartta
            .build_languages(&params, vec![Box::new(Cancelling(Arc::clone(&kartta), at))])
            .map_or_else(
                |error| serde_json::to_value(&error).expect("JSON"),
                |answer| serde_json::from_slice(&answer).expect("JSON"),
            );

        (tree, kartta, built)
    }

    fn asked(kartta: &Kartta, method: &str, params: &str) -> Value {
        serde_json::from_slice(&kartta.call(method, params.as_bytes())).expect("JSON")
    }

    #[test]
    fn a_cancel_in_the_walk_leaves_each_file_query_answering_what_it_reached_and_saying_so() {
        let (_tree, kartta, built) = cancelled_at(CancelAt::FirstFileRead);
        let stopped = serde_json::json!({ "entries": 1, "unvisited": 2 });

        assert_eq!(built["code"], "build_cancelled", "{built}");
        let glob = asked(&kartta, "glob", r#"{"globs": ["**"]}"#);
        assert_eq!(glob["ok"]["paths"], serde_json::json!(["a.defs"]), "{glob}");
        assert_eq!(glob["ok"]["stopped_build"], stopped);
        let list = asked(&kartta, "list", "{}");
        assert_eq!(list["ok"]["total"], 1, "{list}");
        assert_eq!(list["ok"]["stopped_build"], stopped);
        let fuzzy = asked(&kartta, "fuzzy", r#"{"query": "a"}"#);
        assert_eq!(fuzzy["ok"]["matches"][0]["path"], "a.defs", "{fuzzy}");
        assert_eq!(fuzzy["ok"]["stopped_build"], stopped);
        let grep = asked(&kartta, "grep", r#"{"patterns": ["needle"]}"#);
        assert_eq!(grep["ok"]["lines"], 1, "{grep}");
        assert_eq!(grep["ok"]["stopped_build"], stopped);
        let stats = asked(&kartta, "stats", "");
        assert_eq!(stats["ok"]["entries"], 1, "{stats}");
        assert_eq!(stats["ok"]["graphs"], serde_json::json!([]));
        assert_eq!(stats["ok"]["stopped_build"], stopped);
    }

    #[test]
    fn a_method_a_stopped_build_cannot_answer_says_what_the_handle_still_answers() {
        let (_tree, kartta, built) = cancelled_at(CancelAt::FirstFileRead);

        for answer in [
            asked(&kartta, "callers", r#"{"name": "main"}"#),
            asked(&kartta, "refresh", ""),
        ] {
            assert_eq!(answer["err"]["code"], "not_built", "{answer}");
            let message = answer["err"]["message"].as_str().expect("a message");
            assert!(message.contains("glob, list"), "{message}");
            assert!(
                message.contains("the 1 entries its walk reached"),
                "{message}"
            );
            assert!(message.contains("turned 2 away"), "{message}");
        }
        assert!(
            built["message"]
                .as_str()
                .expect("a message")
                .contains("the 1 entries its walk reached"),
            "{built}"
        );
    }

    #[test]
    fn a_path_a_stopped_walk_never_reached_answers_not_found_and_says_the_tree_may_hold_it() {
        let (_tree, kartta, _) = cancelled_at(CancelAt::FirstFileRead);

        let answer = asked(&kartta, "list", r#"{"path": "c/d.defs"}"#);

        assert_eq!(answer["err"]["code"], "not_found", "{answer}");
        let message = answer["err"]["message"].as_str().expect("a message");
        assert!(message.contains("the tree may hold it"), "{message}");
    }

    #[test]
    fn a_cancel_past_the_walk_leaves_the_file_queries_the_whole_tree() {
        let (_tree, kartta, built) = cancelled_at(CancelAt::ResolverBuilt);

        assert_eq!(built["code"], "build_cancelled", "{built}");
        let glob = asked(&kartta, "glob", r#"{"globs": ["**/*.defs"]}"#);
        assert_eq!(
            glob["ok"]["paths"],
            serde_json::json!(["a.defs", "b.defs", "c/d.defs"]),
            "{glob}"
        );
        assert_eq!(
            glob["ok"]["stopped_build"],
            serde_json::json!({ "entries": 4, "unvisited": 0 })
        );
        let list = asked(&kartta, "list", r#"{"path": "missing"}"#);
        let message = list["err"]["message"].as_str().expect("a message");
        assert!(!message.contains("may hold"), "{message}");
        assert_eq!(
            asked(&kartta, "callers", r#"{"name": "main"}"#)["err"]["code"],
            "not_built"
        );
    }

    #[test]
    fn a_cancel_past_the_walk_answers_the_worktrees_the_stopped_index_holds_empty() {
        let (tree, kartta) = handle();
        let git_dir = tree.path().join(".git/worktrees/agent");
        fs::create_dir_all(&git_dir).expect("the worktree's git directory");
        fs::write(git_dir.join("commondir"), "../..\n").expect("commondir");
        let worktree = tree.path().join("agent");
        fs::create_dir_all(&worktree).expect("the worktree");
        fs::write(worktree.join(".git"), "gitdir: ../.git/worktrees/agent\n").expect("the gitfile");
        fs::write(worktree.join("a.defs"), "a\n").expect("the worktree's file");
        fs::write(tree.path().join("a.defs"), "a\n").expect("a.defs");
        let kartta = Arc::new(kartta);
        let params: BuildParams =
            wire::params(br#"{"id": 1, "threads": 1}"#).expect("the params read");
        let cancelling = Cancelling(Arc::clone(&kartta), CancelAt::ResolverBuilt);

        let built = kartta.build_languages(&params, vec![Box::new(cancelling)]);

        assert!(built.is_err(), "the build is cancelled");
        let stats = asked(&kartta, "stats", "");
        assert!(stats["ok"].get("stopped_build").is_some(), "{stats}");
        assert_eq!(
            stats["ok"]["pruned_worktrees"],
            serde_json::json!(["agent"]),
            "{stats}"
        );
    }

    #[test]
    fn a_cancel_after_the_last_file_resolved_answers_build_cancelled_and_every_query_answers() {
        let (_tree, kartta, built) = cancelled_at(CancelAt::Resolving);

        assert_eq!(built["code"], "build_cancelled", "{built}");
        let glob = asked(&kartta, "glob", r#"{"globs": ["**/*.defs"]}"#);
        assert!(glob["ok"].get("stopped_build").is_none(), "{glob}");
        let stats = asked(&kartta, "stats", "");
        assert!(stats["ok"].get("stopped_build").is_none(), "{stats}");
        assert_eq!(stats["ok"]["graphs"][0]["files"], 3, "{stats}");
        assert!(
            asked(&kartta, "callers", r#"{"name": "main", "depth": 3}"#)
                .get("ok")
                .is_some()
        );
    }

    #[test]
    fn a_panic_past_the_walk_loses_the_whole_index_it_held() {
        let (_tree, kartta, built) = cancelled_at(CancelAt::ResolverPanics);

        assert_eq!(built["code"], "internal", "{built}");
        let glob = asked(&kartta, "glob", r#"{"globs": ["**"]}"#);
        assert_eq!(glob["err"]["code"], "not_built", "{glob}");
    }

    #[test]
    fn a_build_replaces_what_a_stopped_build_left() {
        let (_tree, kartta, _) = cancelled_at(CancelAt::FirstFileRead);

        let rebuilt = asked(&kartta, "build", "");

        assert!(rebuilt.get("ok").is_some(), "{rebuilt}");
        assert!(rebuilt["ok"].get("stopped_build").is_none(), "{rebuilt}");
        let glob = asked(&kartta, "glob", r#"{"globs": ["**/*.defs"]}"#);
        assert_eq!(
            glob["ok"]["paths"].as_array().map(Vec::len),
            Some(3),
            "{glob}"
        );
        assert!(glob["ok"].get("stopped_build").is_none(), "{glob}");
    }

    #[test]
    fn a_failed_build_leaves_nothing_of_what_a_stopped_build_left() {
        let (tree, kartta, _) = cancelled_at(CancelAt::FirstFileRead);
        let root = tree.path().to_owned();
        tree.close().expect("the tree goes");

        let failed = asked(&kartta, "build", "");

        assert_eq!(
            failed["err"]["code"],
            "build_failed",
            "{failed} at {}",
            root.display()
        );
        let glob = asked(&kartta, "glob", r#"{"globs": ["**"]}"#);
        assert_eq!(glob["err"]["code"], "not_built", "{glob}");
    }
}
