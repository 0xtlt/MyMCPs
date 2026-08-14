import { type Data } from '@generated/data'
import { type PropsWithChildren } from 'react'
import { type JSONDataTypes } from '@adonisjs/core/types/transformers'
import type { InferFlashData } from '@adonisjs/inertia/types'
import type InertiaMiddleware from '#middleware/inertia_middleware'
import type {} from '@inertiajs/core'

export type InertiaProps<T extends JSONDataTypes = {}> = PropsWithChildren<Data.SharedProps & T>

declare module '@inertiajs/core' {
  interface InertiaConfig {
    sharedPageProps: Data.SharedProps
    flashDataType: InferFlashData<InertiaMiddleware>
  }
}
