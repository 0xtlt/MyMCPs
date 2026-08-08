import { type ComponentType, type SVGProps, useMemo, useState } from 'react'
import { Badge } from '@astryxdesign/core/Badge'
import { useAppShellMobile } from '@astryxdesign/core/AppShell'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { EmptyState } from '@astryxdesign/core/EmptyState'
import { Grid } from '@astryxdesign/core/Grid'
import { Icon } from '@astryxdesign/core/Icon'
import { HStack, StackItem, VStack } from '@astryxdesign/core/Layout'
import { Tab, TabList, TabMenu } from '@astryxdesign/core/TabList'
import { Heading, Text } from '@astryxdesign/core/Text'
import { TextInput } from '@astryxdesign/core/TextInput'
import { emptyMcpFormValues, type McpFormValues } from '~/components/mcp_form_fields'

type TemplateCategory = 'productivity' | 'development' | 'commerce' | 'infrastructure'
type TemplateFilter = 'popular' | 'all' | TemplateCategory
type TemplateLogo =
  'notion' | 'shopify' | 'github' | 'linear' | 'stripe' | 'vercel' | 'supabase' | 'cloudflare'

export type McpTemplate = {
  id: string
  name: string
  description: string
  category: TemplateCategory
  popular: boolean
  logo: TemplateLogo
  keywords: string[]
  values: Partial<McpFormValues>
}

type Props = {
  onSelect: (template: McpTemplate, values: McpFormValues) => void
}

const categoryLabels: Record<TemplateCategory, string> = {
  productivity: 'Productivity',
  development: 'Development',
  commerce: 'Commerce',
  infrastructure: 'Infrastructure',
}

const filters: Array<{ value: TemplateFilter; label: string }> = [
  { value: 'popular', label: 'Popular' },
  { value: 'all', label: 'All' },
  { value: 'productivity', label: 'Productivity' },
  { value: 'development', label: 'Development' },
  { value: 'commerce', label: 'Commerce' },
  { value: 'infrastructure', label: 'Infrastructure' },
]

function NotionLogo(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" {...props}>
      <path d="M4.459 4.208c.746.606 1.026.56 2.428.466l13.215-.793c.28 0 .047-.28-.046-.326L17.86 1.968c-.42-.326-.981-.7-2.055-.607L3.01 2.295c-.466.046-.56.28-.374.466zm.793 3.08v13.904c0 .747.373 1.027 1.214.98l14.523-.84c.841-.046.935-.56.935-1.167V6.354c0-.606-.233-.933-.748-.887l-15.177.887c-.56.047-.747.327-.747.933zm14.337.745c.093.42 0 .84-.42.888l-.7.14v10.264c-.608.327-1.168.514-1.635.514-.748 0-.935-.234-1.495-.933l-4.577-7.186v6.952L12.21 19s0 .84-1.168.84l-3.222.186c-.093-.186 0-.653.327-.746l.84-.233V9.854L7.822 9.76c-.094-.42.14-1.026.793-1.073l3.456-.233 4.764 7.279v-6.44l-1.215-.139c-.093-.514.28-.887.747-.933zM1.936 1.035l13.31-.98c1.634-.14 2.055-.047 3.082.7l4.249 2.986c.7.513.934.653.934 1.213v16.378c0 1.026-.373 1.634-1.68 1.726l-15.458.934c-.98.047-1.448-.093-1.962-.747l-3.129-4.06c-.56-.747-.793-1.306-.793-1.96V2.667c0-.839.374-1.54 1.447-1.632z" />
    </svg>
  )
}

function ShopifyLogo(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" {...props}>
      <path d="M15.337 23.979l7.216-1.561s-2.604-17.613-2.625-17.73c-.018-.116-.114-.192-.211-.192s-1.929-.136-1.929-.136-1.275-1.274-1.439-1.411c-.045-.037-.075-.057-.121-.074l-.914 21.104h.023zM11.71 11.305s-.81-.424-1.774-.424c-1.447 0-1.504.906-1.504 1.141 0 1.232 3.24 1.715 3.24 4.629 0 2.295-1.44 3.76-3.406 3.76-2.354 0-3.54-1.465-3.54-1.465l.646-2.086s1.245 1.066 2.28 1.066c.675 0 .975-.545.975-.932 0-1.619-2.654-1.694-2.654-4.359-.034-2.237 1.571-4.416 4.827-4.416 1.257 0 1.875.361 1.875.361l-.945 2.715-.02.01zM11.17.83c.136 0 .271.038.405.135-.984.465-2.064 1.639-2.508 3.992-.656.213-1.293.405-1.889.578C7.697 3.75 8.951.84 11.17.84V.83zm1.235 2.949v.135c-.754.232-1.583.484-2.394.736.466-1.777 1.333-2.645 2.085-2.971.193.501.309 1.176.309 2.1zm.539-2.234c.694.074 1.141.867 1.429 1.755-.349.114-.735.231-1.158.366v-.252c0-.752-.096-1.371-.271-1.871v.002zm2.992 1.289c-.02 0-.06.021-.078.021s-.289.075-.714.21c-.423-1.233-1.176-2.37-2.508-2.37h-.115C12.135.209 11.669 0 11.265 0 8.159 0 6.675 3.877 6.21 5.846c-1.194.365-2.063.636-2.16.674-.675.213-.694.232-.772.87-.075.462-1.83 14.063-1.83 14.063L15.009 24l.927-21.166z" />
    </svg>
  )
}

function GitHubLogo(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" {...props}>
      <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
    </svg>
  )
}

function LinearLogo(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" {...props}>
      <path d="M2.886 4.18A11.982 11.982 0 0 1 11.99 0C18.624 0 24 5.376 24 12.009c0 3.64-1.62 6.903-4.18 9.105L2.887 4.18ZM1.817 5.626l16.556 16.556c-.524.33-1.075.62-1.65.866L.951 7.277c.247-.575.537-1.126.866-1.65ZM.322 9.163l14.515 14.515c-.71.172-1.443.282-2.195.322L0 11.358a12 12 0 0 1 .322-2.195Zm-.17 4.862 9.823 9.824a12.02 12.02 0 0 1-9.824-9.824Z" />
    </svg>
  )
}

function StripeLogo(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" {...props}>
      <path d="M13.976 9.15c-2.172-.806-3.356-1.426-3.356-2.409 0-.831.683-1.305 1.901-1.305 2.227 0 4.515.858 6.09 1.631l.89-5.494C18.252.975 15.697 0 12.165 0 9.667 0 7.589.654 6.104 1.872 4.56 3.147 3.757 4.992 3.757 7.218c0 4.039 2.467 5.76 6.476 7.219 2.585.92 3.445 1.574 3.445 2.583 0 .98-.84 1.545-2.354 1.545-1.875 0-4.965-.921-6.99-2.109l-.9 5.555C5.175 22.99 8.385 24 11.714 24c2.641 0 4.843-.624 6.328-1.813 1.664-1.305 2.525-3.236 2.525-5.732 0-4.128-2.524-5.851-6.594-7.305h.003z" />
    </svg>
  )
}

function VercelLogo(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" {...props}>
      <path d="m12 1.608 12 20.784H0Z" />
    </svg>
  )
}

function SupabaseLogo(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" {...props}>
      <path d="M11.9 1.036c-.015-.986-1.26-1.41-1.874-.637L.764 12.05C-.33 13.427.65 15.455 2.409 15.455h9.579l.113 7.51c.014.985 1.259 1.408 1.873.636l9.262-11.653c1.093-1.375.113-3.403-1.645-3.403h-9.642z" />
    </svg>
  )
}

function CloudflareLogo(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" {...props}>
      <path d="M16.5088 16.8447c.1475-.5068.0908-.9707-.1553-1.3154-.2246-.3164-.6045-.499-1.0615-.5205l-8.6592-.1123a.1559.1559 0 0 1-.1333-.0713c-.0283-.042-.0351-.0986-.021-.1553.0278-.084.1123-.1484.2036-.1562l8.7359-.1123c1.0351-.0489 2.1601-.8868 2.5537-1.9136l.499-1.3013c.0215-.0561.0293-.1128.0147-.168-.5625-2.5463-2.835-4.4453-5.5499-4.4453-2.5039 0-4.6284 1.6177-5.3876 3.8614-.4927-.3658-1.1187-.5625-1.794-.499-1.2026.119-2.1665 1.083-2.2861 2.2856-.0283.31-.0069.6128.0635.894C1.5683 13.171 0 14.7754 0 16.752c0 .1748.0142.3515.0352.5273.0141.083.0844.1475.1689.1475h15.9814c.0909 0 .1758-.0645.2032-.1553l.12-.4268zm2.7568-5.5634c-.0771 0-.1611 0-.2383.0112-.0566 0-.1054.0415-.127.0976l-.3378 1.1744c-.1475.5068-.0918.9707.1543 1.3164.2256.3164.6055.498 1.0625.5195l1.8437.1133c.0557 0 .1055.0263.1329.0703.0283.043.0351.1074.0214.1562-.0283.084-.1132.1485-.204.1553l-1.921.1123c-1.041.0488-2.1582.8867-2.5527 1.914l-.1406.3585c-.0283.0713.0215.1416.0986.1416h6.5977c.0771 0 .1474-.0489.169-.126.1122-.4082.1757-.837.1757-1.2803 0-2.6025-2.125-4.727-4.7344-4.727" />
    </svg>
  )
}

const logoIcons: Record<TemplateLogo, ComponentType<SVGProps<SVGSVGElement>>> = {
  notion: NotionLogo,
  shopify: ShopifyLogo,
  github: GitHubLogo,
  linear: LinearLogo,
  stripe: StripeLogo,
  vercel: VercelLogo,
  supabase: SupabaseLogo,
  cloudflare: CloudflareLogo,
}

export const mcpTemplates: McpTemplate[] = [
  {
    id: 'notion',
    name: 'Notion',
    description: 'Search, read, and update pages and databases in your Notion workspace.',
    category: 'productivity',
    popular: true,
    logo: 'notion',
    keywords: ['notes', 'docs', 'wiki', 'database'],
    values: {
      name: 'Notion',
      description: 'Search, read, and update pages and databases in Notion.',
      transport: 'http',
      httpUrl: 'https://mcp.notion.com/mcp',
      authType: 'auto',
    },
  },
  {
    id: 'shopify-dev',
    name: 'Shopify Dev',
    description: 'Build Shopify apps with current API, Liquid, and Polaris development guidance.',
    category: 'commerce',
    popular: true,
    logo: 'shopify',
    keywords: ['store', 'ecommerce', 'polaris', 'liquid'],
    values: {
      name: 'Shopify Dev',
      description:
        'Shopify development tools and documentation for apps, APIs, Liquid, and Polaris.',
      transport: 'npm',
      npmPackage: '@shopify/dev-mcp',
      npmVersion: 'latest',
      authType: 'auto',
    },
  },
  {
    id: 'github',
    name: 'GitHub',
    description: 'Work with repositories, issues, pull requests, code, and GitHub Actions.',
    category: 'development',
    popular: true,
    logo: 'github',
    keywords: ['git', 'repository', 'pull request', 'actions'],
    values: {
      name: 'GitHub',
      description: 'Work with GitHub repositories, issues, pull requests, code, and Actions.',
      transport: 'http',
      httpUrl: 'https://api.githubcopilot.com/mcp/',
      authType: 'auto',
    },
  },
  {
    id: 'linear',
    name: 'Linear',
    description: 'Find, create, and update issues, projects, and comments in Linear.',
    category: 'productivity',
    popular: true,
    logo: 'linear',
    keywords: ['issues', 'projects', 'roadmap', 'tasks'],
    values: {
      name: 'Linear',
      description: 'Find, create, and update issues, projects, and comments in Linear.',
      transport: 'http',
      httpUrl: 'https://mcp.linear.app/mcp',
      authType: 'auto',
    },
  },
  {
    id: 'stripe',
    name: 'Stripe',
    description: 'Interact with Stripe payments, customers, products, and developer documentation.',
    category: 'commerce',
    popular: false,
    logo: 'stripe',
    keywords: ['payments', 'billing', 'customers', 'finance'],
    values: {
      name: 'Stripe',
      description: 'Interact with Stripe payments, customers, products, and documentation.',
      transport: 'http',
      httpUrl: 'https://mcp.stripe.com',
      authType: 'auto',
    },
  },
  {
    id: 'vercel',
    name: 'Vercel',
    description: 'Inspect projects, deployments, logs, domains, and Vercel documentation.',
    category: 'infrastructure',
    popular: true,
    logo: 'vercel',
    keywords: ['deployments', 'hosting', 'logs', 'domains'],
    values: {
      name: 'Vercel',
      description: 'Inspect Vercel projects, deployments, logs, domains, and documentation.',
      transport: 'http',
      httpUrl: 'https://mcp.vercel.com',
      authType: 'auto',
    },
  },
  {
    id: 'supabase',
    name: 'Supabase',
    description: 'Manage databases, projects, functions, debugging, and Supabase documentation.',
    category: 'development',
    popular: false,
    logo: 'supabase',
    keywords: ['postgres', 'database', 'functions', 'backend'],
    values: {
      name: 'Supabase',
      description: 'Manage Supabase databases, projects, functions, debugging, and documentation.',
      transport: 'http',
      httpUrl: 'https://mcp.supabase.com/mcp',
      authType: 'auto',
    },
  },
  {
    id: 'cloudflare-docs',
    name: 'Cloudflare Docs',
    description: 'Search current Cloudflare developer documentation for Workers and the platform.',
    category: 'infrastructure',
    popular: false,
    logo: 'cloudflare',
    keywords: ['workers', 'platform', 'documentation', 'edge'],
    values: {
      name: 'Cloudflare Docs',
      description:
        'Search current Cloudflare developer documentation for Workers and the platform.',
      transport: 'http',
      httpUrl: 'https://docs.mcp.cloudflare.com/mcp',
      authType: 'auto',
    },
  },
]

export function mcpTemplateFormValues(template: McpTemplate): McpFormValues {
  return { ...emptyMcpFormValues(), ...template.values }
}

export function McpTemplateGallery({ onSelect }: Props) {
  const { isMobile } = useAppShellMobile()
  const [activeFilter, setActiveFilter] = useState<TemplateFilter>('popular')
  const [query, setQuery] = useState('')

  const visibleTemplates = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase()

    return mcpTemplates.filter((template) => {
      const matchesCategory =
        activeFilter === 'all' ||
        (activeFilter === 'popular' ? template.popular : template.category === activeFilter)
      const searchableText = [
        template.name,
        template.description,
        categoryLabels[template.category],
        ...template.keywords,
      ]
        .join(' ')
        .toLocaleLowerCase()

      return matchesCategory && searchableText.includes(normalizedQuery)
    })
  }, [activeFilter, query])

  function clearFilters() {
    setActiveFilter('all')
    setQuery('')
  }

  return (
    <VStack gap={5} hAlign="stretch">
      <TextInput
        label="Search templates"
        value={query}
        onChange={(value) => {
          setQuery(value)
          if (value.trim().length > 0) setActiveFilter('all')
        }}
        placeholder="Search Notion, Shopify, GitHub…"
        startIcon="search"
        hasClear
        isLabelHidden
        width="100%"
      />

      <TabList value={activeFilter} onChange={(value) => setActiveFilter(value as TemplateFilter)}>
        {isMobile ? (
          <>
            <Tab value="popular" label="Popular" />
            <Tab value="all" label="All" />
            <TabMenu label="Categories" options={filters.slice(2)} />
          </>
        ) : (
          filters.map((filter) => (
            <Tab key={filter.value} value={filter.value} label={filter.label} />
          ))
        )}
      </TabList>

      <Text type="supporting" color="secondary">
        {visibleTemplates.length} {visibleTemplates.length === 1 ? 'template' : 'templates'}
      </Text>

      {visibleTemplates.length > 0 ? (
        <Grid columns={{ minWidth: 240, repeat: 'fit' }} gap={4} width="100%">
          {visibleTemplates.map((template) => {
            const Logo = logoIcons[template.logo]

            return (
              <Card
                key={template.id}
                minHeight="calc(var(--spacing-12) + var(--spacing-12) + var(--spacing-12) + var(--spacing-12) + var(--spacing-12))"
                padding={5}
              >
                <VStack gap={4} hAlign="stretch" height="100%">
                  <HStack gap={3} vAlign="start">
                    <Icon icon={Logo} size="lg" color="primary" />
                    <StackItem size="fill">
                      <VStack gap={1} hAlign="start">
                        <Heading level={3}>{template.name}</Heading>
                        <Text type="supporting" color="secondary">
                          {categoryLabels[template.category]}
                        </Text>
                      </VStack>
                    </StackItem>
                    {template.popular ? <Badge label="Popular" variant="purple" /> : null}
                  </HStack>

                  <StackItem size="fill">
                    <Text type="body" color="secondary" maxLines={3}>
                      {template.description}
                    </Text>
                  </StackItem>

                  <HStack hAlign="end">
                    <Button
                      label={`Set up ${template.name}`}
                      variant="primary"
                      size="sm"
                      onClick={() => onSelect(template, mcpTemplateFormValues(template))}
                    />
                  </HStack>
                </VStack>
              </Card>
            )
          })}
        </Grid>
      ) : (
        <EmptyState
          title="No templates found"
          description="Try another search or clear the active category."
          icon={<Icon icon="search" size="lg" />}
          actions={<Button label="Clear filters" variant="secondary" onClick={clearFilters} />}
          headingLevel={3}
          isCompact
        />
      )}
    </VStack>
  )
}
