import { useEffect } from 'react';
import { Alert } from 'react-native';
import * as Linking from 'expo-linking';
import * as WebBrowser from 'expo-web-browser';
import { router } from 'expo-router';
import { normalizeJarvisPairingUrl } from '../lib/linking';
import { parsePairingString } from '../lib/parse-pairing';
import { usePairingDeepLink } from './PairingDeepLinkContext';

/**
 * Handles jarvis://pair?... and completes any in-flight auth sessions from the system browser.
 */
export default function DeepLinkListener() {
  const { queuePairingUrl } = usePairingDeepLink();

  useEffect(() => {
    WebBrowser.maybeCompleteAuthSession();
  }, []);

  useEffect(() => {
    const handleUrl = (url: string) => {
      const pairing = normalizeJarvisPairingUrl(url);
      if (!pairing) return;

      let relayHost: string;
      try {
        const parsed = parsePairingString(pairing);
        relayHost = new URL(parsed.relayUrl).hostname;
      } catch {
        return;
      }

      Alert.alert(
        'Connect to relay?',
        'Pair with ' + relayHost + '?',
        [
          { text: 'Cancel', style: 'cancel' },
          {
            text: 'Connect',
            onPress: () => {
              queuePairingUrl(pairing);
              router.replace('/');
            },
          },
        ]
      );
    };

    const sub = Linking.addEventListener('url', ({ url }) => handleUrl(url));
    void Linking.getInitialURL().then((u) => {
      if (u) handleUrl(u);
    });
    return () => sub.remove();
  }, [queuePairingUrl]);

  return null;
}
