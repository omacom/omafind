//! The indexer: crawl the chosen folders under the rules, read what changed, embed it, store it. And the search:
//! embed the query, ask the store, rank.

pub mod crawler;
pub mod indexer;
pub mod rules;
pub mod search;
pub mod survey;

pub use crawler::{Candidate, Root, crawl};
pub use indexer::{Event, Indexer};
pub use search::{
    Answer, SearchOptions, Timings, code_query, hits_for, plan, prompted, rank, search, similar,
};
