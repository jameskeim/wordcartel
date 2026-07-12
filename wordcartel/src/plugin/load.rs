//! The filesystem-free load core (`load_sources`, Task 4) and the filesystem/config discovery
//! layer (`discover`, Task 6) that drives it: exec each plugin source into the host VM, cap +
//! intern its registrations, and commit them into the `Registry` atomically per plugin. Task 2
//! is a stub — nothing loads yet.
