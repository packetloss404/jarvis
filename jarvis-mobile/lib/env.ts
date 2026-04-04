/**
 * Build-time public env (Expo). No secrets here.
 */
export function getDefaultRelayHint(): string | undefined {
  const v = process.env.EXPO_PUBLIC_DEFAULT_RELAY_URL;
  return typeof v === 'string' && v.trim() ? v.trim() : undefined;
}

export function getSupabaseUrlHint(): string | undefined {
  const v = process.env.EXPO_PUBLIC_SUPABASE_URL;
  return typeof v === 'string' && v.trim() ? v.trim() : undefined;
}
