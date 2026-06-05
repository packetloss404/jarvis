import React, { useRef, useCallback, useState } from 'react';
import { View, Platform, KeyboardAvoidingView, Text } from 'react-native';
import { WebView, WebViewMessageEvent } from 'react-native-webview';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { buildChatHTML } from '../lib/jarvis-chat-html';
import { getChatRelayUrl } from '../lib/env';
import { theme, scaledFont } from '../lib/theme';
import { useWebViewAndroidBack } from '../hooks/useWebViewAndroidBack';

const mono = Platform.OS === 'ios' ? 'Menlo' : 'monospace';

/**
 * Inject the relay URL the chat HTML's RoomConnection reads (window.__JARVIS_RELAY_URL__).
 * This mirrors the desktop, which provides relayUrl via the chat_stream_control IPC; in the
 * WebView there is no Rust IPC, so we set the global before the chat bundle runs.
 */
function buildRelayInjection(relayUrl: string): string {
  return `window.__JARVIS_RELAY_URL__ = ${JSON.stringify(relayUrl)}; true;`;
}

export default function JarvisChatWebView() {
  const webViewRef = useRef<WebView>(null);
  const insets = useSafeAreaInsets();
  const htmlRef = useRef(buildChatHTML());
  const relayInjectionRef = useRef(buildRelayInjection(getChatRelayUrl()));
  const { onNavigationStateChange } = useWebViewAndroidBack(webViewRef);
  const [httpError, setHttpError] = useState<string | null>(null);

  const handleMessage = useCallback((event: WebViewMessageEvent) => {
    try {
      const data = JSON.parse(event.nativeEvent.data);
      if (data.type === 'ready') {
        // Livechat HTML has initialized — it connects to the relay Room on its own
      }
    } catch {
      // ignore parse errors
    }
  }, []);

  return (
    <KeyboardAvoidingView
      style={{ flex: 1, backgroundColor: theme.colors.bg }}
      behavior={Platform.OS === 'ios' ? 'padding' : undefined}
      keyboardVerticalOffset={insets.top}
    >
      <View style={{ flex: 1, paddingTop: insets.top }}>
        {httpError ? (
          <View style={{ paddingHorizontal: 10, paddingVertical: 6, backgroundColor: 'rgba(60, 30, 10, 0.4)' }}>
            <Text style={{ fontFamily: mono, fontSize: scaledFont(10), color: '#ffcc88' }}>
              Livechat: network or HTTP issue ({httpError}). Check Wi‑Fi, VPN, and relay reachability.
            </Text>
          </View>
        ) : null}
        <WebView
          ref={webViewRef}
          source={{ html: htmlRef.current }}
          injectedJavaScriptBeforeContentLoaded={relayInjectionRef.current}
          style={{ flex: 1, backgroundColor: 'transparent' }}
          originWhitelist={['*']}
          javaScriptEnabled
          domStorageEnabled
          scrollEnabled={false}
          keyboardDisplayRequiresUserAction={false}
          onMessage={handleMessage}
          onNavigationStateChange={onNavigationStateChange}
          onLoadEnd={() => setHttpError(null)}
          onHttpError={(e) => setHttpError(String(e.nativeEvent.statusCode))}
          onError={() => setHttpError('load failed')}
        />
      </View>
    </KeyboardAvoidingView>
  );
}
