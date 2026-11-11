// Text and text inputs as the whole app uses them.
//
// Text follows the person's text-size setting, up to a limit: the amounts, pills and the signer's
// small screen are laid out for a size, and beyond about a third larger they overlap or clip.
// Inputs get a dark keyboard to match the app, and keyboards with no key that puts them away
// (number pads, and fields where return starts a new line) a Done button above the keyboard, in
// the system's own accessory bar.
import React, { createContext, useContext, useId, useRef } from 'react';
import {
  InputAccessoryView, Keyboard, Platform, Pressable, Text as NativeText, TextInput as NativeTextInput,
  TextInputProps, TextProps, View,
} from 'react-native';
import { colors } from '../theme';

const LARGEST = 1.3;
const padsWithoutReturn = ['number-pad', 'decimal-pad', 'numeric', 'phone-pad'];

// What holds an input (a sheet, say) hears which one has the keyboard, to keep it in view.
export const FocusContext = createContext<((input: NativeTextInput) => void) | null>(null);

export function Text(props: TextProps & { ref?: React.Ref<NativeText> }) {
  return <NativeText maxFontSizeMultiplier={LARGEST} {...props}/>;
}

export function TextInput({ keyboardType, inputAccessoryViewID, ref, onFocus, ...props }: TextInputProps & { ref?: React.Ref<NativeTextInput> }) {
  const needsDone = Platform.OS === 'ios' && !inputAccessoryViewID && (props.multiline || padsWithoutReturn.includes(keyboardType ?? ''));
  // Each input brings its own bar: iOS ties a bar to the inputs that exist when it appears,
  // so one shared bar would miss inputs shown later, or in a sheet.
  const bar = useId();
  const focused = useContext(FocusContext);
  const input = useRef<NativeTextInput | null>(null);
  const keep = (node: NativeTextInput | null) => {
    input.current = node;
    if (typeof ref === 'function') ref(node); else if (ref) ref.current = node;
  };
  return <>
    <NativeTextInput ref={keep} maxFontSizeMultiplier={LARGEST} keyboardAppearance="dark" selectionColor={colors.mint}
      keyboardType={keyboardType} inputAccessoryViewID={inputAccessoryViewID ?? (needsDone ? bar : undefined)}
      onFocus={event => { if (input.current) focused?.(input.current); onFocus?.(event); }} {...props}/>
    {needsDone && <InputAccessoryView nativeID={bar}>
      <View style={{ flexDirection: 'row', justifyContent: 'flex-end', paddingHorizontal: 8, backgroundColor: 'rgba(40,40,46,.94)', borderTopWidth: .5, borderTopColor: colors.border }}>
        <Pressable accessibilityRole="button" accessibilityLabel="Done" onPress={Keyboard.dismiss} hitSlop={6}
          style={({ pressed }) => ({ paddingVertical: 11, paddingHorizontal: 12, opacity: pressed ? .6 : 1 })}>
          <NativeText maxFontSizeMultiplier={LARGEST} style={{ color: colors.mint, fontSize: 16, fontWeight: '600' }}>Done</NativeText>
        </Pressable>
      </View>
    </InputAccessoryView>}
  </>;
}
