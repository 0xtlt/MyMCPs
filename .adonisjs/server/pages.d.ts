import '@adonisjs/inertia/types'

import type React from 'react'
import type { Prettify } from '@adonisjs/core/types/common'

type ExtractProps<T> =
  T extends React.FC<infer Props>
    ? Prettify<Omit<Props, 'children'>>
    : T extends React.Component<infer Props>
      ? Prettify<Omit<Props, 'children'>>
      : never

declare module '@adonisjs/inertia/types' {
  export interface InertiaPages {
    'analytics/index': ExtractProps<(typeof import('../../inertia/pages/analytics/index.tsx'))['default']>
    'auth/login': ExtractProps<(typeof import('../../inertia/pages/auth/login.tsx'))['default']>
    'errors/not_found': ExtractProps<(typeof import('../../inertia/pages/errors/not_found.tsx'))['default']>
    'errors/server_error': ExtractProps<(typeof import('../../inertia/pages/errors/server_error.tsx'))['default']>
    'home': ExtractProps<(typeof import('../../inertia/pages/home.tsx'))['default']>
    'invites/accept': ExtractProps<(typeof import('../../inertia/pages/invites/accept.tsx'))['default']>
    'invites/index': ExtractProps<(typeof import('../../inertia/pages/invites/index.tsx'))['default']>
    'logs/index': ExtractProps<(typeof import('../../inertia/pages/logs/index.tsx'))['default']>
    'mcps/index': ExtractProps<(typeof import('../../inertia/pages/mcps/index.tsx'))['default']>
    'oauth/authorize': ExtractProps<(typeof import('../../inertia/pages/oauth/authorize.tsx'))['default']>
    'onboarding/index': ExtractProps<(typeof import('../../inertia/pages/onboarding/index.tsx'))['default']>
    'settings/index': ExtractProps<(typeof import('../../inertia/pages/settings/index.tsx'))['default']>
    'tokens/index': ExtractProps<(typeof import('../../inertia/pages/tokens/index.tsx'))['default']>
  }
}
