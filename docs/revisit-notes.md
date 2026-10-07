# Storage Decision Notes

The RAG storage rethink is implemented. AtomDB now durably owns two isolated
RAG roots: neutral general retrieval and bounded self-learning. Native AtomDB
passage/term bonds drive retrieval directly; only its derived graph index and
the active-document catalog rebuild in memory at startup.

SQLite remains the practical store for non-RAG runtime state. The retired
relational RAG projection and importer have been removed, and a forward
migration drops its old tables. AtomDB is the sole RAG database.

Future revisit areas are peer-native AtomDB replication, compaction policy for
long event chains, and whether other durable memory lanes merit their own
AtomDB roots. Those are separate decisions and should not silently merge
self-learning with general retrieval.
