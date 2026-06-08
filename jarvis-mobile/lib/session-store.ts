import * as SecureStore from 'expo-secure-store';

// Key must be alphanumeric + dot/underscore/hyphen, max 240 chars.
// Former AsyncStorage key was '@jarvis/claude_session_token' — sanitised here.
const SESSION_TOKEN_KEY = 'jarvis.claude_session_token';

export async function loadSessionToken(): Promise<string | null> {
  try {
    return await SecureStore.getItemAsync(SESSION_TOKEN_KEY);
  } catch {
    return null;
  }
}

export async function saveSessionToken(token: string): Promise<void> {
  await SecureStore.setItemAsync(SESSION_TOKEN_KEY, token).catch(() => {});
}

export async function clearSessionToken(): Promise<void> {
  await SecureStore.deleteItemAsync(SESSION_TOKEN_KEY).catch(() => {});
}
