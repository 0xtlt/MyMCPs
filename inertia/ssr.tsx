import { client } from '~/client'
import Layout from '~/layouts/default'
import ReactDOMServer from 'react-dom/server'
import { createInertiaApp, type ResolvedComponent } from '@inertiajs/react'
import { TuyauProvider } from '@adonisjs/inertia/react'
import { resolvePageComponent } from '@adonisjs/inertia/helpers'

export default function render(page: any) {
  return createInertiaApp({
    page,
    render: ReactDOMServer.renderToString,
    resolve: async (name) => {
      const resolved = await resolvePageComponent(
        `./pages/${name}.tsx`,
        import.meta.glob<{ default: ResolvedComponent }>('./pages/**/*.tsx', { eager: true }),
        Layout
      )
      return resolved.default
    },
    setup: ({ App, props }) => {
      return (
        <TuyauProvider client={client}>
          <App {...props} />
        </TuyauProvider>
      )
    },
  })
}
