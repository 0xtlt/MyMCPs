import { useState } from 'react'
import { Head } from '@inertiajs/react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { Center } from '@astryxdesign/core/Center'
import { FileInput } from '@astryxdesign/core/FileInput'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { Link } from '@astryxdesign/core/Link'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'

/** What the server takes. It checks again: this only saves sending a file for nothing. */
const MAX_BACKUP_BYTES = 4 * 1024 ** 3

type ImportForm = {
  backup: File | null
  password: string
  /** Never sent: the name under which a refusal that is about no field comes back. */
  import?: never
}

export default function OnboardingImport() {
  const [backup, setBackup] = useState<File | null>(null)
  const [password, setPassword] = useState('')

  return (
    <Center axis="both" style={{ minHeight: '100%' }}>
      <Head title="Import a backup" />
      <Card padding={8} maxWidth={420} width="100%">
        {/* The file input keeps its file in this page's state: it joins the fields on the way out. */}
        <Form<ImportForm>
          action={{ url: '/onboarding/import', method: 'post' }}
          transform={(fields) => ({ ...fields, backup })}
        >
          {({ errors, processing }) => (
            <VStack gap={4} hAlign="stretch">
              <VStack gap={1}>
                <Heading level={1}>Import a backup</Heading>
                <Text type="body" color="secondary">
                  Restore the users, MCPs, access tokens, call logs and settings of another MyMCPs
                  instance.
                </Text>
              </VStack>

              {/* A refusal that is about no field: another import is running. */}
              {errors.import ? (
                <Banner status="error" title={errors.import} container="card" />
              ) : null}

              <FileInput
                label="Backup file"
                value={backup}
                onChange={(file) => setBackup(Array.isArray(file) ? (file[0] ?? null) : file)}
                accept=".mymcps"
                maxSize={MAX_BACKUP_BYTES}
                width="100%"
                status={errors.backup ? { type: 'error', message: errors.backup } : undefined}
              />

              <TextInput
                label="Backup password"
                type="password"
                htmlName="password"
                value={password}
                onChange={setPassword}
                autoComplete="off"
                size="lg"
                width="100%"
                status={errors.password ? { type: 'error', message: errors.password } : undefined}
              />

              {/* An import can take a while: the button says what is going on. */}
              <Button
                type="submit"
                label={processing ? 'Importing…' : 'Import backup'}
                variant="primary"
                size="lg"
                width="100%"
                isDisabled={processing}
              />

              <HStack hAlign="center">
                <Link href="/onboarding" isStandalone>
                  Create a new instance instead
                </Link>
              </HStack>
            </VStack>
          )}
        </Form>
      </Card>
    </Center>
  )
}
