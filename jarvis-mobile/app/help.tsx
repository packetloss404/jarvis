import { ScrollView, Text, TouchableOpacity, Platform, View } from 'react-native';
import { useRouter } from 'expo-router';
import { theme, scaledFont } from '../lib/theme';

const mono = Platform.OS === 'ios' ? 'Menlo' : 'monospace';

function Section({ title, body }: { title: string; body: string }) {
  return (
    <View style={{ marginBottom: 20 }}>
      <Text
        style={{
          fontFamily: mono,
          fontSize: scaledFont(11),
          color: theme.colors.primarySolid,
          marginBottom: 8,
        }}
      >
        {title}
      </Text>
      <Text style={{ fontFamily: mono, fontSize: scaledFont(10), color: theme.colors.text, lineHeight: scaledFont(16) }}>
        {body}
      </Text>
    </View>
  );
}

export default function HelpScreen() {
  const router = useRouter();

  return (
    <ScrollView
      style={{ flex: 1, backgroundColor: theme.colors.bg }}
      contentContainerStyle={{ padding: 16, paddingBottom: 48 }}
    >
      <Text
        style={{
          fontFamily: mono,
          fontSize: scaledFont(12),
          color: theme.colors.text,
          marginBottom: 16,
        }}
      >
        Jarvis mobile — three separate transports (no shared session between them).
      </Text>

      <Section
        title="[ code ] Relay terminal"
        body={
          'Pairs with desktop Jarvis over a relay WebSocket. Paste pairing text, scan a QR, or open a jarvis://pair?… link. ' +
          'Traffic is encrypted with ECDH + AES-GCM after the desktop joins the same session. ' +
          'If you stay on “connecting”, check the relay URL, desktop app, and firewall. ' +
          'Red banner errors mean the socket or crypto setup failed — try disconnect and pair again.'
        }
      />

      <Section
        title="[ chat ] Livechat"
        body={
          'Runs the livechat UI inside a WebView and talks to the relay Room transport (one WebSocket per channel) from the page. ' +
          'If the panel shows a connection error, you are offline or blocked from the relay host — the same host the relay terminal uses. ' +
          'Override the relay with EXPO_PUBLIC_DEFAULT_RELAY_URL at build time (defaults to the production relay).'
        }
      />

      <Section
        title="[ claude ] Claude Code"
        body={
          'Loads claude.ai in a WebView. Many providers block embedded OAuth; use [browser] to sign in with the system browser, ' +
          'then return and tap [reload]. expo-web-browser may complete some auth sessions when redirects match your app scheme.'
        }
      />

      <Section
        title="Accessibility & type"
        body={
          'UI font sizes respect system font scale up to a cap so the terminal row stays usable. ' +
          'Tab bar entries have accessibility labels for screen readers.'
        }
      />

      <Section
        title="Developer: relay debug"
        body={
          'Set EXPO_PUBLIC_RELAY_DEBUG=1 to show the last relay message type on the code tab (no payload content logged).'
        }
      />

      <TouchableOpacity onPress={() => router.back()} style={{ marginTop: 8 }}>
        <Text style={{ fontFamily: mono, fontSize: scaledFont(11), color: theme.colors.primary }}>[ close ]</Text>
      </TouchableOpacity>
    </ScrollView>
  );
}
