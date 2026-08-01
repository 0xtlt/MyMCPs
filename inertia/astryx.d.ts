import type { HTMLInputAutoCompleteAttribute } from 'react'

declare module '@astryxdesign/core/TextInput' {
  interface TextInputProps {
    autoComplete?: HTMLInputAutoCompleteAttribute
  }
}
