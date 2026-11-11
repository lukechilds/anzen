// Keeping the field being typed in above the keyboard, on a scrolling screen.
//
// iOS does this itself (`automaticallyAdjustKeyboardInsets`). Android does not: its keyboard is
// drawn over the app, which is told how tall it is and nothing more. So there the screen gets
// that much more room to scroll while the keyboard is up, and scrolls the focused field into view.
import { useEffect, useRef, useState } from 'react';
import { Keyboard, Platform, ScrollView, TextInput } from 'react-native';

// `pad` is extra room for the foot of the scroll view's content; `focused` goes to FocusContext
// around its inputs.
export function useKeyboardReveal(scroll: React.RefObject<ScrollView | null>) {
  const [pad, setPad] = useState(0);
  const input = useRef<TextInput | null>(null);
  const up = useRef(false);
  const reveal = () => {
    const field = input.current;
    if (!up.current || !field?.isFocused()) return;
    scroll.current?.getScrollResponder()?.scrollResponderScrollNativeHandleToKeyboard(field as never, 96, true);
  };
  useEffect(() => {
    if (Platform.OS !== 'android') return;
    const listeners = [
      // The room to scroll arrives with the next render.
      Keyboard.addListener('keyboardDidShow', event => { up.current = true; setPad(event.endCoordinates.height); setTimeout(reveal, 80); }),
      Keyboard.addListener('keyboardDidHide', () => { up.current = false; setPad(0); }),
    ];
    return () => listeners.forEach(listener => listener.remove());
  }, []);
  // Moving to another field with the keyboard already up.
  const focused = (field: TextInput) => { input.current = field; setTimeout(reveal, 80); };
  return { pad, focused };
}
