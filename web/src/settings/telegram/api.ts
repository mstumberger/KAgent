// The telegram routes (api.md section 6, bots.md) and what they answer. The settings
// themselves are in `Settings.telegram` (api.ts).
import { api, type Settings } from "../../api";

/** `settings.telegram` (config.md section 11): off until the owner pairs a chat. */
export type TelegramSettings = NonNullable<Settings["telegram"]>;

/** The polling state of the bot (bots.md section 6); `telegramStatus` events carry the same names. */
export type TelegramState = "off" | "disabled" | "unpaired" | "polling" | "error";

/** A pairing code still waiting: six digits, ten minutes, one use. */
export interface PairingCode {
  code: string;
  expiresAt: string;
}

/** `GET /api/telegram`. It says whether a token is stored, never the token. */
export interface TelegramStatus {
  enabled: boolean;
  configured: boolean;
  status: TelegramState;
  /** The bot the token belongs to; null until Telegram was asked. */
  botUsername: string | null;
  paired: boolean;
  chatId: number | null;
  /** RFC 3339. */
  pairedAt: string | null;
  /** The last error of the poll, kept after the bot recovered; null when there was none. */
  lastError: string | null;
  pendingCode: PairingCode | null;
}

export const telegramApi = {
  status: () => api.get<TelegramStatus>("/api/telegram"),
  /** The five settings; the answer is the status after the change. */
  save: (patch: TelegramSettings) => api.put<TelegramStatus>("/api/telegram", patch),
  /** Kept only when Telegram knows the token; answers with the bot's username. */
  setToken: (token: string) => api.post<{ botUsername: string }>("/api/telegram/token", { token }),
  /** Also unpairs. */
  deleteToken: () => api.del<void>("/api/telegram/token"),
  pairingCode: () => api.post<PairingCode>("/api/telegram/pairing-code", {}),
  unpair: () => api.del<void>("/api/telegram/pairing"),
  test: () => api.post<{ ok: boolean; error?: string }>("/api/telegram/test", {}),
};
