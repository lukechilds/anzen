// Sheets, as iOS presents them: a real UIKit sheet that slides up over the app, sized to what
// it holds, with the system's grabber, dimming and swipe-to-dismiss. The browser emulator draws
// its own (Sheet.web.tsx).
import React, { useEffect, useId, useRef, useState } from 'react';
import { BackHandler, Keyboard, Platform, ScrollView, StyleSheet, TextInput, useWindowDimensions, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { ScreenStack, ScreenStackItem } from 'react-native-screens';
import { FocusContext } from './Text';
import { colors } from '../theme';

// The app's screen, and the sheets over it. Sheets are presented by the native stack they are
// children of, so they are passed apart from what they cover.
export function SheetHost({ children, sheets }: { children: React.ReactNode; sheets: React.ReactNode }) {
  return <ScreenStack style={{ flex: 1 }}>
    <ScreenStackItem screenId="app" activityState={2} headerConfig={{ hidden: true }} style={StyleSheet.absoluteFill}>
      <View style={{ flex: 1 }}>{children}</View>
    </ScreenStackItem>
    {sheets}
  </ScreenStack>;
}

// Space kept around a focused input when it is scrolled into view, and the sheet's own padding.
const MARGIN = 14, PADDING = 20;

// A sheet as tall as its content, scrolling once that is taller than the screen allows.
// `onClose` runs when the person swipes it away or taps beside it; unmounting it closes it too.
//
// The system lifts a sheet clear of the keyboard as far as the screen allows, and no further:
// a tall sheet is left with its lower part behind the keyboard. Whatever the keyboard still
// covers once it is up becomes room to scroll, and the input being typed in is kept on screen.
export function Sheet({ children, onClose, label }: { children: React.ReactNode; onClose: () => void; label: string }) {
  const id = useId();
  const window = useWindowDimensions(), insets = useSafeAreaInsets();
  const [content, setContent] = useState(0);
  // How much of the sheet's foot the keyboard covers.
  const [covered, setCovered] = useState(0);
  const tallest = window.height - insets.top - 44;
  const scroll = useRef<ScrollView>(null), inner = useRef<View>(null);
  const now = useRef({ offset: 0, frame: 0, covered: 0, content: 0, input: null as TextInput | null });
  now.current.content = content;
  const reveal = () => {
    const input = now.current.input;
    if (!now.current.covered || !input?.isFocused() || !inner.current) return;
    input.measureLayout(inner.current, (_x, y, _width, tall) => {
      const { offset, frame, covered: hidden } = now.current, top = y + PADDING, visible = frame - hidden;
      if (top + tall + MARGIN > offset + visible) scroll.current?.scrollTo({ y: Math.max(0, Math.min(top - MARGIN, top + tall + MARGIN - visible)), animated: true });
      else if (top - MARGIN < offset) scroll.current?.scrollTo({ y: Math.max(0, top - MARGIN), animated: true });
    }, () => {});
  };
  useEffect(() => {
    const listeners = [
      // Measured once the keyboard and the sheet have both settled.
      // (A scroll view's ref is its native view, with the scrolling methods added.)
      Keyboard.addListener('keyboardDidShow', event => (scroll.current as unknown as View | null)?.measureInWindow((_x, y, _width, tall) => {
        const hidden = Math.max(0, Math.round(y + tall - event.endCoordinates.screenY));
        now.current.frame = tall; now.current.covered = hidden;
        setCovered(hidden);
        // The room to scroll arrives with the next render.
        setTimeout(reveal, 30);
      })),
      // (Android tells of the keyboard only once it has gone.)
      Keyboard.addListener(Platform.OS === 'ios' ? 'keyboardWillHide' : 'keyboardDidHide', () => {
        const { offset, frame, content: tall } = now.current;
        now.current.covered = 0; setCovered(0);
        // Nothing is left scrolled past its end once the room to scroll is gone.
        if (offset > Math.max(0, tall - frame)) scroll.current?.scrollTo({ y: Math.max(0, tall - frame), animated: true });
      }),
    ];
    return () => listeners.forEach(listener => listener.remove());
  }, []);
  // Android's Back closes the sheet on top, as swiping it away does. (The newest listener is asked first.)
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const back = BackHandler.addEventListener('hardwareBackPress', () => { close.current(); return true; });
    return () => back.remove();
  }, []);
  // Moving to another input with the keyboard already up: there is no keyboard event to wait for.
  const focused = (input: TextInput) => { now.current.input = input; setTimeout(reveal, 60); };
  return <ScreenStackItem screenId={id} activityState={2} stackPresentation="formSheet" sheetAllowedDetents="fitToContents"
    sheetGrabberVisible sheetCornerRadius={38} headerConfig={{ hidden: true }} onDismissed={onClose}
    // iOS gives a sheet its own material. Android gives it nothing, and what is behind shows through.
    contentStyle={Platform.OS === 'android' ? { backgroundColor: colors.surface } : undefined}
    accessibilityLabel={label} accessibilityViewIsModal>
    <ScrollView ref={scroll} style={content ? { height: Math.min(content, tallest) } : undefined} onContentSizeChange={(_, tall) => setContent(Math.ceil(tall))}
      // iOS takes the covered part as an inset; Android has no such inset, so there it is padding.
      scrollEnabled={content > tallest || covered > 0} contentInset={{ bottom: covered }} scrollIndicatorInsets={{ bottom: covered }}
      onScroll={event => { now.current.offset = event.nativeEvent.contentOffset.y; }} scrollEventThrottle={32}
      keyboardShouldPersistTaps="handled" keyboardDismissMode="interactive"
      showsVerticalScrollIndicator={false} contentContainerStyle={{ paddingHorizontal: 22, paddingTop: PADDING, paddingBottom: 22 + (Platform.OS === 'ios' ? 0 : covered) }}>
      <View ref={inner} collapsable={false}><FocusContext.Provider value={focused}>{children}</FocusContext.Provider></View>
    </ScrollView>
  </ScreenStackItem>;
}
