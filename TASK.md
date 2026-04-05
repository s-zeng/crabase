Recreate Obsidian CLI's `obsidian base:query format=csv file=...` command as a standalone rust application.

There is a sample call using my existing obsidian vault in `REPRO.md`

The complete documentation for Obsidian bases is contained in ./bases_docs/

This project will require you to implement a parser for the Bases yaml format, and to be able to conduct the appropriate filters and aggregations in the vault to produce the same results.
