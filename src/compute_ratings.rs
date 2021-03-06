// Copy-paste a spreadsheet column of CF handles as input to this program, then
// paste this program's output into the spreadsheet's ratings column.
use std::cmp::max;
use std::collections::{VecDeque, HashSet, HashMap};
use std::fs::File;
use std::io;
use std::str;

const NUM_TITLES: usize = 11;
const TITLE_BOUND: [i32; NUM_TITLES] = [-999,1000,1200,1400,1600,1800,2000,2200,2400,2700,3000];
const TITLE: [&str; NUM_TITLES] = ["Ne","Pu","Ap","Sp","Ex","CM","Ma","IM","GM","IG","LG"];
const SIG_LIMIT: f64 = 100.0; // limiting uncertainty for a player who competed a lot
const SIG_PERF: f64 = 250.0; // variation in individual performances
const SIG_NEWBIE: f64 = 350.0; // uncertainty for a new player
const SIX_MONTHS_AGO: usize = 1131;

struct Scanner<R> {
    reader: R,
    buf_str: Vec<u8>,
    buf_iter: str::SplitAsciiWhitespace<'static>,
}

impl<R: io::BufRead> Scanner<R> {
    fn new(reader: R) -> Self {
        Self { reader, buf_str: Vec::new(), buf_iter: "".split_ascii_whitespace() }
    }
    fn token<T: str::FromStr>(&mut self) -> T {
        loop {
            if let Some(token) = self.buf_iter.next() {
                return token.parse().ok().expect("Failed parse");
            }
            self.buf_str.clear();
            self.reader.read_until(b'\n', &mut self.buf_str).expect("Failed read");
            self.buf_iter = unsafe {
                let slice = str::from_utf8_unchecked(&self.buf_str);
                std::mem::transmute(slice.split_ascii_whitespace())
            }
        }
    }
}

fn scanner_from_file(filename: &str) -> Scanner<io::BufReader<std::fs::File>> {
    let file = File::open(filename).expect("Input file not found");
    Scanner::new(io::BufReader::new(file))
}

fn writer_to_file(filename: &str) -> io::BufWriter<std::fs::File> {
    let file = std::fs::File::create(filename).expect("Output file not found");
    io::BufWriter::new(file)
}

fn get_contests() -> Vec<usize> {
    let mut team_contests = HashSet::new();
    let mut solo_contests = Vec::new();
    
    let mut scan = scanner_from_file("../data/team_contests.txt");
    for _ in 0..scan.token::<usize>() {
        let contest = scan.token::<usize>();
        team_contests.insert(contest);
    }
    
    scan = scanner_from_file("../data/all_contests.txt");
    for _ in 0..scan.token::<usize>() {
        let contest = scan.token::<usize>();
        if !team_contests.contains(&contest) {
            solo_contests.push(contest);
        }
    }
    
    assert_eq!(team_contests.len(), 17);
    assert_eq!(solo_contests.len(), 948);
    solo_contests
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Rating {
    mu: f64,
    sig: f64,
}

impl Default for Rating {
    fn default() -> Self {
        Rating {
            mu: 1500.0,
            sig: SIG_NEWBIE,
        }
    }
}

#[derive(Default)]
struct Player {
    normal_factor: Rating,
    logistic_factors: VecDeque<Rating>,
    approx_posterior: Rating,
    max_rating: i32,
    last_rating: i32,
    last_contest: usize,
}

impl Player {
    // apply noise to one variable for which we have many estimates
    fn add_noise_uniform(&mut self, sig_noise: f64) {
        // conveniently update the last rating before applying noise for the next contest
        self.last_rating = self.conservative_rating();
        
        // multiply all sigmas by the same decay
        let decay = 1.0f64.hypot(sig_noise / self.approx_posterior.sig);
        self.normal_factor.sig *= decay;
        for rating in &mut self.logistic_factors {
            rating.sig *= decay;
        }
        self.approx_posterior.sig *= decay;
    }
    
    fn recompute_posterior(&mut self) {
        let mut sig_inv_sq = self.normal_factor.sig.powi(-2);
        let logistic_vec = self.logistic_factors.iter().cloned().collect::<Vec<_>>();
        let mu = robust_mean(&logistic_vec, -self.normal_factor.mu*sig_inv_sq, sig_inv_sq);
        for &factor in &self.logistic_factors {
            sig_inv_sq += factor.sig.powi(-2);
        }
        self.approx_posterior = Rating{ mu, sig: sig_inv_sq.recip().sqrt() };
    }
    
    fn add_performance(&mut self, perf: f64) {
        if self.logistic_factors.len() == 50_000 {
            let logistic = self.logistic_factors.pop_front().unwrap();
            let wn = self.normal_factor.sig.powi(-2);
            let wl = logistic.sig.powi(-2);
            self.normal_factor.mu = (self.normal_factor.mu * wn + logistic.mu * wl) / (wn + wl);
            self.normal_factor.sig = (wn + wl).recip().sqrt();
        }
        self.logistic_factors.push_back(Rating { mu: perf, sig: SIG_PERF });

        self.recompute_posterior();
        self.max_rating = max(self.max_rating, self.conservative_rating());
    }
    
    fn conservative_rating(&self) -> i32 {
        (self.approx_posterior.mu - 2.0 * (self.approx_posterior.sig - SIG_LIMIT)).round() as i32
    }
}

// returns something near the mean if the ratings are consistent; near the median if they're far apart
// offC and offM are constant and slope offsets, respectively
fn robust_mean(all_ratings: &[Rating], off_c: f64, off_m: f64) -> f64 {
    let (mut lo, mut hi) = (-1000.0, 4500.0);
    while hi - lo > 1e-9 {
        let mid = 0.5 * (lo + hi);
        let mut sum = off_c + off_m * mid;
        for &rating in all_ratings {
            sum += ((mid - rating.mu) / rating.sig).tanh() / rating.sig;
        }
        if sum > 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    0.5 * (lo + hi)
}

// ratings is a list of the participants, ordered from first to last place
// returns: performance of the player in ratings[id] who tied against ratings[lo..hi]
fn performance(better: &[Rating], worse: &[Rating], all: &[Rating]) -> f64 {
    let pos_offset: f64 = better.iter().map(|rating| rating.sig.recip()).sum();
    let neg_offset: f64 = worse.iter().map(|rating| rating.sig.recip()).sum();
    robust_mean(all, pos_offset - neg_offset, 0.0)
}

fn simulate_contest(players: &mut HashMap<String, Player>, contest: usize) {
    let sig_noise = ( (SIG_LIMIT.powi(-2) - SIG_PERF.powi(-2)).recip() - SIG_LIMIT.powi(2) ).sqrt();
    
    let filename = format!("../standings/{}.txt", contest);
    let mut scan = scanner_from_file(&filename);
    let num_contestants = scan.token::<usize>();
    let title = scan.buf_iter.by_ref().collect::<Vec<_>>().join(" ");
    println!("Processing {} contestants in contest/{}: {}", num_contestants, contest, title);
    
    let mut results = Vec::with_capacity(num_contestants);
    let mut all_ratings = Vec::with_capacity(num_contestants + 1);
    for _ in 0..num_contestants {
        let handle = scan.token::<String>();
        let rank_lo = scan.token::<usize>() - 1;
        let rank_hi = scan.token::<usize>() - 1;
        results.push((handle.clone(), rank_lo, rank_hi));
        
        let player = players.entry(handle).or_default();
        player.add_noise_uniform(sig_noise);
        let rating = player.approx_posterior;
        all_ratings.push(Rating { mu: rating.mu, sig: rating.sig.hypot(SIG_PERF)  } );
    }
    
    // begin rating updates
    for (i, (handle, lo, hi)) in results.into_iter().enumerate() {
        assert!(lo <= i && i <= hi && hi < num_contestants);
        let extra_rating = all_ratings[i];
        all_ratings.push(extra_rating);
        let perf = performance(&all_ratings[0..lo], &all_ratings[hi+1..num_contestants], &all_ratings);
        all_ratings.pop();
        
        let player = players.get_mut(&handle).expect("Couldn't find player");
        player.add_performance(perf);
        player.last_contest = contest;
    }
    // end rating updates
}

struct RatingData {
    cur_rating: i32,
    max_rating: i32,
    handle: String,
    last_contest: usize,
    last_perf: i32,
    last_delta: i32,
}

fn print_ratings(players: &HashMap<String, Player>) {
    use io::Write;
    let mut out = writer_to_file("../data/CFratings_temp.txt");
    let recent_contests: HashSet<usize> = get_contests().into_iter()
                         .skip_while(|&i| i != SIX_MONTHS_AGO).collect();
    
    let mut sum_ratings = 0.0;
    let mut rating_data = Vec::with_capacity(players.len());
    let mut title_count = vec![0; NUM_TITLES];
    for (handle, player) in players {
        sum_ratings += player.approx_posterior.mu;
        let cur_rating = player.conservative_rating();
        let max_rating = player.max_rating;
        let handle = handle.clone();
        let last_contest = player.last_contest;
        let last_perf = player.logistic_factors.back().unwrap().mu.round() as i32;
        let last_delta = cur_rating - player.last_rating;
        rating_data.push(RatingData {
            cur_rating,
            max_rating,
            handle,
            last_contest,
            last_perf,
            last_delta,
        });
        
        if recent_contests.contains(&last_contest) {
            if let Some(title_id) = (0..NUM_TITLES).rev().find(|&i| cur_rating >= TITLE_BOUND[i]) {
                title_count[title_id] += 1;
            }
        }
    }
    rating_data.sort_unstable_by_key(|data| -data.cur_rating);
    
    writeln!(out, "Mean rating.mu = {}", sum_ratings / players.len() as f64).ok();
    
    for i in (0..NUM_TITLES).rev() {
        writeln!(out, "{} {} x {}", TITLE_BOUND[i], TITLE[i], title_count[i]).ok();
    }
    
    let mut rank = 0;
    for data in rating_data {
        if recent_contests.contains(&data.last_contest) {
            rank += 1;
            write!(out, "{:6}", rank).ok();
        } else {
            write!(out, "{:>6}", "-").ok();
        }
        write!(out, " {:4}({:4})", data.cur_rating, data.max_rating).ok();
        write!(out, " {:<26}contest/{:4}: ", data.handle, data.last_contest).ok();
        writeln!(out, "perf ={:5}, delta ={:4}", data.last_perf, data.last_delta).ok();
    }
}

fn main() {
    // simulates the entire history of Codeforces, runs on my laptop in two hours
    let mut players = HashMap::<String, Player>::new();
    for contest in get_contests() {
        simulate_contest(&mut players, contest);
    }
    print_ratings(&players);
}
