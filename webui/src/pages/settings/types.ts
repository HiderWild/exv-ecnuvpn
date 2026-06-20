import type { Component } from 'vue'

export type SectionKey = 'auth' | 'connection' | 'personalization' | 'system' | 'routes'

export interface SettingsSectionDescriptor {
  key: SectionKey
  label: string
  icon: Component
}
