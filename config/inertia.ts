import app from '@adonisjs/core/services/app'
import { defineConfig } from '@adonisjs/inertia'

const inertiaConfig = defineConfig({
  /**
   * Encrypt the page props the browser keeps in its history, such as a
   * one-time access token, with a key that is dropped on the login page.
   * Browsers only expose the required Web Crypto API on HTTPS and localhost;
   * elsewhere Inertia cannot navigate at all once this is on. Production
   * already needs such an origin for its secure cookies, a development
   * server opened over plain HTTP on a LAN address does not.
   */
  encryptHistory: !app.inDev,

  /**
   * Server-side rendering options.
   */
  ssr: {
    /**
     * Toggle SSR mode for Inertia pages.
     */
    enabled: false,

    /**
     * Entry file used by the SSR server build.
     */
    entrypoint: 'inertia/ssr.tsx',
  },
})

export default inertiaConfig
