/**
 * @deprecated Import from `lib/commands/` sub-modules for tree-shaking.
 * This barrel re-exports the most commonly used commands.
 */
export {
  getSessions,
  createSession,
  deleteSession,
} from './commands/session';
export {
  getMessages,
  sendMessage,
  resolveAgentConfirmation,
  preflightPendingDesktopAction,
} from './commands/message';
export { pauseAgent, resumeAgent, interruptAgent, submitInProgressCommand } from './commands/steering';
export {
  getMcpServers,
  saveMcpServer,
  deleteMcpServer,
  testApiConnection,
  checkOllama,
} from './commands/mcp';
export {
  getMemories,
  saveMemory,
  deleteMemory,
  getProfile,
  saveProfile,
} from './commands/memory';
export {
  getScheduledTasks,
  saveScheduledTask,
  deleteScheduledTask,
  getEnvConfig,
  matchPersonalityDirection,
  exportData,
  importData,
  type EnvProviderInfo,
  type PersonalityMatchResult,
} from './commands/settings';
