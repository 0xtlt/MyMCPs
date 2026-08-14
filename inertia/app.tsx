import './css/app.css'
import { client } from './client'
import Layout from '~/layouts/default'
import { createRoot } from 'react-dom/client'
import { createInertiaApp, type ResolvedComponent } from '@inertiajs/react'
import { Link as InertiaLink, TuyauProvider } from '@adonisjs/inertia/react'
import { resolvePageComponent } from '@adonisjs/inertia/helpers'
import { Theme } from '@astryxdesign/core/theme'
import { LinkProvider } from '@astryxdesign/core/Link'
import { neutralTheme } from '@astryxdesign/theme-neutral/built'

const appName = import.meta.env.VITE_APP_NAME || 'MyMCPs'

createInertiaApp({
  title: (title) => (title ? `${title} - ${appName}` : appName),
  resolve: async (name) => {
    const resolved = await resolvePageComponent(
      `./pages/${name}.tsx`,
      import.meta.glob<{ default: ResolvedComponent }>('./pages/**/*.tsx'),
      Layout
    )
    return resolved.default
  },
  setup({ el, App, props }) {
    createRoot(el).render(
      <Theme theme={neutralTheme}>
        <LinkProvider component={InertiaLink}>
          <TuyauProvider client={client}>
            <App {...props} />
          </TuyauProvider>
        </LinkProvider>
      </Theme>
    )
  },
  progress: {
    color: '#4B5563',
  },
})
