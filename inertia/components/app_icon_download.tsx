import { useState } from 'react'
import { Button } from '@astryxdesign/core/Button'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { Text } from '@astryxdesign/core/Text'
import logoUrl from '~/assets/brand/mymcps-m-logo.png?w=512&format=png&img'

const ICON_SIZE = 512
const ICON_FILENAME = 'mymcps-app-icon.png'

/**
 * Draw the MyMCPs mark on an opaque white square. The logo itself is
 * transparent, which a provider that converts uploads to JPG shows on black.
 */
async function generateAppIcon() {
  const logo = new Image()
  logo.src = logoUrl
  await logo.decode()

  const canvas = document.createElement('canvas')
  canvas.width = ICON_SIZE
  canvas.height = ICON_SIZE
  const context = canvas.getContext('2d', { alpha: false })
  if (!context) {
    throw new Error('Canvas is not available')
  }
  context.fillStyle = '#ffffff'
  context.fillRect(0, 0, ICON_SIZE, ICON_SIZE)
  context.drawImage(logo, 0, 0, ICON_SIZE, ICON_SIZE)

  return new Promise<Blob>((resolve, reject) => {
    canvas.toBlob(
      (blob) => (blob ? resolve(blob) : reject(new Error('The icon could not be encoded'))),
      'image/png'
    )
  })
}

function saveFile(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = filename
  document.body.append(link)
  link.click()
  link.remove()
  // The download reads the blob after the click returns.
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

/**
 * A ready-made icon for providers that require one when an API application is
 * registered. It is generated in the browser, so nothing is uploaded anywhere.
 */
export function AppIconDownload() {
  const [hasFailed, setHasFailed] = useState(false)

  async function download() {
    try {
      saveFile(await generateAppIcon(), ICON_FILENAME)
      setHasFailed(false)
    } catch {
      setHasFailed(true)
    }
  }

  return (
    <VStack gap={2} hAlign="start">
      <HStack gap={3} vAlign="center">
        <img src={logoUrl} alt="" className="app-logo" />
        <Button label="Download app icon" variant="secondary" size="sm" onClick={download} />
      </HStack>
      {hasFailed ? (
        <Text type="supporting" color="secondary">
          The icon could not be generated in this browser. Any JPG or PNG image works instead.
        </Text>
      ) : null}
    </VStack>
  )
}
