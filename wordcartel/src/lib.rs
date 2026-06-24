#![forbid(unsafe_code)]
//! Wordcartel terminal shell (imperative shell over wordcartel-core).
pub mod editor;
pub mod derive;
pub mod nav;
pub mod commands;
pub mod input;
pub mod registry;
pub mod render;
pub mod term;
pub mod file;
pub mod jobs;
pub mod app;   // App::run + the testable App::step (Task 12)
