# AgentStack

- Start with `agentstack status`; change the manifest or linked library, never generated MCP configs or skill folders, and preview every write.
- In live mode, browse skills with `agentstack_list_loadable`, load only the needed body with `agentstack_load`, and discover MCP tools with `tools_search`; missing native capability files are intentional.
- Keep secrets as `${REF}`, refresh the lock after selected content changes, leave `agentstack trust .` to the human, and load the built-in `using-agentstack` skill for detailed operations.
