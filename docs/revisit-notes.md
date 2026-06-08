# Revisit Notes

## Storage
- Revisit SQLite after the current local-first baseline is stable.
- Explore an alternative databank shape that better matches the long-term
  product vision, especially around:
  - distributed state
  - peer-native replication
  - user preference for a less conventional storage model
  - separation between runtime memory state and user-owned file space
  - long-lived self-heal memory that may eventually want a different retention
    model from general RAG and working memory

## Current stance
- SQLite is the current practical baseline because it is embedded, local, easy
  to migrate, and works well while the rest of the architecture is still moving.
- This is an implementation choice, not a final philosophical commitment.
