import React, { useRef, useCallback, useEffect, useState } from 'react';
import { View, Text, Platform } from 'react-native';
import { useRouter } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { theme } from '../lib/theme';
import TerminalWebView, { TerminalWebViewHandle } from './TerminalWebView';
import SessionTokenInput from './SessionTokenInput';
import PairingQrScanner from './PairingQrScanner';
import { useRelayConnection } from '../hooks/useRelayConnection';
import { usePairingDeepLink } from '../contexts/PairingDeepLinkContext';
import type { PaneInfo } from '../lib/relay-connection';

const paneMono = Platform.OS === 'ios' ? 'Menlo' : 'monospace';

function PaneIndicator({ panes, activePaneId }: { panes: PaneInfo[]; activePaneId: number }) {
  if (panes.length <= 1) return null;
  const activeIdx = panes.findIndex(p => p.id === activePaneId);
  const activePane = panes[activeIdx];
  return (
    <View style={{
      flexDirection: 'row',
      justifyContent: 'center',
      alignItems: 'center',
      paddingVertical: 6,
      gap: 6,
    }}>
      {panes.map((p, i) => (
        <View
          key={p.id}
          style={{
            width: 6,
            height: 6,
            borderRadius: 3,
            backgroundColor: i === activeIdx
              ? theme.colors.primary
              : 'rgba(0, 212, 255, 0.2)',
          }}
        />
      ))}
      {activePane && (
        <Text style={{
          color: 'rgba(0, 212, 255, 0.5)',
          fontFamily: paneMono,
          fontSize: 10,
          marginLeft: 4,
        }}>
          {activePane.title}
        </Text>
      )}
    </View>
  );
}

export default function CodeTerminal() {
  const insets = useSafeAreaInsets();
  const router = useRouter();
  const terminalRef = useRef<TerminalWebViewHandle>(null);
  const [qrOpen, setQrOpen] = useState(false);
  const { pendingPairingUrl, clearPendingPairingUrl } = usePairingDeepLink();

  const {
    status,
    sessionToken,
    terminalReady,
    connect,
    disconnect,
    onTerminalReady,
    onTerminalInput,
    onTerminalResize,
    panes,
    activePaneId,
    switchToNext,
    switchToPrev,
  } = useRelayConnection(terminalRef);

  useEffect(() => {
    if (!terminalReady || !pendingPairingUrl) return;
    const p = pendingPairingUrl;
    clearPendingPairingUrl();
    void connect(p);
  }, [terminalReady, pendingPairingUrl, connect, clearPendingPairingUrl]);

  const handleSwipe = useCallback((direction: 'prev' | 'next') => {
    if (direction === 'prev') switchToPrev();
    else switchToNext();
  }, [switchToPrev, switchToNext]);

  const openSettings = useCallback(() => {
    router.push('/settings');
  }, [router]);

  return (
    <View style={{ flex: 1, backgroundColor: theme.colors.bg, paddingTop: insets.top }}>
      <SessionTokenInput
        status={status}
        currentToken={sessionToken}
        onConnect={connect}
        onDisconnect={disconnect}
        onScanPress={() => setQrOpen(true)}
        onSettingsPress={openSettings}
      />
      <PaneIndicator panes={panes} activePaneId={activePaneId} />
      <TerminalWebView
        ref={terminalRef}
        onReady={onTerminalReady}
        onInput={onTerminalInput}
        onResize={onTerminalResize}
        onSwipe={handleSwipe}
      />
      <PairingQrScanner
        visible={qrOpen}
        onClose={() => setQrOpen(false)}
        onPairingScanned={(data) => void connect(data)}
      />
    </View>
  );
}
