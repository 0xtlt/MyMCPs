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
  | 'notion'
  | 'shopify'
  | 'github'
  | 'linear'
  | 'stripe'
  | 'vercel'
  | 'supabase'
  | 'cloudflare'
  | 'atlassian'
  | 'postman'
  | 'sentry'
  | 'microsoft'
  | 'firebase'
  | 'mongodb'
  | 'neon'
  | 'figma'
  | 'canva'
  | 'huggingface'
  | 'context7'
  | 'slack'

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

function createBrandLogo(path: string) {
  return function BrandLogo(props: SVGProps<SVGSVGElement>) {
    return (
      <svg viewBox="0 0 24 24" {...props}>
        <path d={path} />
      </svg>
    )
  }
}

const AtlassianLogo = createBrandLogo(
  'M7.12 11.084a.683.683 0 00-1.16.126L.075 22.974a.703.703 0 00.63 1.018h8.19a.678.678 0 00.63-.39c1.767-3.65.696-9.203-2.406-12.52zM11.434.386a15.515 15.515 0 00-.906 15.317l3.95 7.9a.703.703 0 00.628.388h8.19a.703.703 0 00.63-1.017L12.63.38a.664.664 0 00-1.196.006z'
)
const PostmanLogo = createBrandLogo(
  'M13.527.099C6.955-.744.942 3.9.099 10.473c-.843 6.572 3.8 12.584 10.373 13.428 6.573.843 12.587-3.801 13.428-10.374C24.744 6.955 20.101.943 13.527.099zm2.471 7.485a.855.855 0 0 0-.593.25l-4.453 4.453-.307-.307-.643-.643c4.389-4.376 5.18-4.418 5.996-3.753zm-4.863 4.861 4.44-4.44a.62.62 0 1 1 .847.903l-4.699 4.125-.588-.588zm.33.694-1.1.238a.06.06 0 0 1-.067-.032.06.06 0 0 1 .01-.073l.645-.645.512.512zm-2.803-.459 1.172-1.172.879.878-1.979.426a.074.074 0 0 1-.085-.039.072.072 0 0 1 .013-.093zm-3.646 6.058a.076.076 0 0 1-.069-.083.077.077 0 0 1 .022-.046h.002l.946-.946 1.222 1.222-2.123-.147zm2.425-1.256a.228.228 0 0 0-.117.256l.203.865a.125.125 0 0 1-.211.117h-.003l-.934-.934-.294-.295 3.762-3.758 1.82-.393.874.874c-1.255 1.102-2.971 2.201-5.1 3.268zm5.279-3.428h-.002l-.839-.839 4.699-4.125a.952.952 0 0 0 .119-.127c-.148 1.345-2.029 3.245-3.977 5.091zm3.657-6.46-.003-.002a1.822 1.822 0 0 1 2.459-2.684l-1.61 1.613a.119.119 0 0 0 0 .169l1.247 1.247a1.817 1.817 0 0 1-2.093-.343zm2.578 0a1.714 1.714 0 0 1-.271.218h-.001l-1.207-1.207 1.533-1.533c.661.72.637 1.832-.054 2.522zM18.855 6.05a.143.143 0 0 0-.053.157.416.416 0 0 1-.053.45.14.14 0 0 0 .023.197.141.141 0 0 0 .084.03.14.14 0 0 0 .106-.05.691.691 0 0 0 .087-.751.138.138 0 0 0-.194-.033z'
)
const SentryLogo = createBrandLogo(
  'M13.91 2.505c-.873-1.448-2.972-1.448-3.844 0L6.904 7.92a15.478 15.478 0 0 1 8.53 12.811h-2.221A13.301 13.301 0 0 0 5.784 9.814l-2.926 5.06a7.65 7.65 0 0 1 4.435 5.848H2.194a.365.365 0 0 1-.298-.534l1.413-2.402a5.16 5.16 0 0 0-1.614-.913L.296 19.275a2.182 2.182 0 0 0 .812 2.999 2.24 2.24 0 0 0 1.086.288h6.983a9.322 9.322 0 0 0-3.845-8.318l1.11-1.922a11.47 11.47 0 0 1 4.95 10.24h5.915a17.242 17.242 0 0 0-7.885-15.28l2.244-3.845a.37.37 0 0 1 .504-.13c.255.14 9.75 16.708 9.928 16.9a.365.365 0 0 1-.327.543h-2.287c.029.612.029 1.223 0 1.831h2.297a2.206 2.206 0 0 0 1.922-3.31z'
)
const MicrosoftLogo = createBrandLogo(
  'M0 0v11.408h11.408V0zm12.594 0v11.408H24V0zM0 12.594V24h11.408V12.594zm12.594 0V24H24V12.594z'
)
const FirebaseLogo = createBrandLogo(
  'M19.455 8.369c-.538-.748-1.778-2.285-3.681-4.569-.826-.991-1.535-1.832-1.884-2.245a146 146 0 0 0-.488-.576l-.207-.245-.113-.133-.022-.032-.01-.005L12.57 0l-.609.488c-1.555 1.246-2.828 2.851-3.681 4.64-.523 1.064-.864 2.105-1.043 3.176-.047.241-.088.489-.121.738-.209-.017-.421-.028-.632-.033-.018-.001-.035-.002-.059-.003a7.46 7.46 0 0 0-2.28.274l-.317.089-.163.286c-.765 1.342-1.198 2.869-1.252 4.416-.07 2.01.477 3.954 1.583 5.625 1.082 1.633 2.61 2.882 4.42 3.611l.236.095.071.025.003-.001a9.59 9.59 0 0 0 2.941.568q.171.006.342.006c1.273 0 2.513-.249 3.69-.742l.008.004.313-.145a9.63 9.63 0 0 0 3.927-3.335c1.01-1.49 1.577-3.234 1.641-5.042.075-2.161-.643-4.304-2.133-6.371m-7.083 6.695c.328 1.244.264 2.44-.191 3.558-1.135-1.12-1.967-2.352-2.475-3.665-.543-1.404-.87-2.74-.974-3.975.48.157.922.366 1.315.622 1.132.737 1.914 1.902 2.325 3.461zm.207 6.022c.482.368.99.712 1.513 1.028-.771.21-1.565.302-2.369.273a8 8 0 0 1-.373-.022c.458-.394.869-.823 1.228-1.279zm1.347-6.431c-.516-1.957-1.527-3.437-3.002-4.398-.647-.421-1.385-.741-2.194-.95.011-.134.026-.268.043-.4.014-.113.03-.216.046-.313.133-.689.332-1.37.589-2.025.099-.25.206-.499.321-.74l.004-.008c.177-.358.376-.719.61-1.105l.092-.152-.003-.001c.544-.851 1.197-1.627 1.942-2.311l.288.341c.672.796 1.304 1.548 1.878 2.237 1.291 1.549 2.966 3.583 3.612 4.48 1.277 1.771 1.893 3.579 1.83 5.375-.049 1.395-.461 2.755-1.195 3.933-.694 1.116-1.661 2.05-2.8 2.708-.636-.318-1.559-.839-2.539-1.599.79-1.575.952-3.28.479-5.072zm-2.575 5.397c-.725.939-1.587 1.55-2.09 1.856-.081-.029-.163-.06-.243-.093l-.065-.026c-1.49-.616-2.747-1.656-3.635-3.01-.907-1.384-1.356-2.993-1.298-4.653.041-1.19.338-2.327.882-3.379.316-.07.638-.114.96-.131l.084-.002c.162-.003.324-.003.478 0 .227.011.454.035.677.07.073 1.513.445 3.145 1.105 4.852.637 1.644 1.694 3.162 3.144 4.515z'
)
const MongoDbLogo = createBrandLogo(
  'M17.193 9.555c-1.264-5.58-4.252-7.414-4.573-8.115-.28-.394-.53-.954-.735-1.44-.036.495-.055.685-.523 1.184-.723.566-4.438 3.682-4.74 10.02-.282 5.912 4.27 9.435 4.888 9.884l.07.05A73.49 73.49 0 0111.91 24h.481c.114-1.032.284-2.056.51-3.07.417-.296.604-.463.85-.693a11.342 11.342 0 003.639-8.464c.01-.814-.103-1.662-.197-2.218zm-5.336 8.195s0-8.291.275-8.29c.213 0 .49 10.695.49 10.695-.381-.045-.765-1.76-.765-2.405z'
)
const NeonLogo = createBrandLogo(
  'M24 0V24l-9.365-8.045V24H0V0ZM2.942 21.087h8.751V9.563l9.365 8.204V2.919L2.942 2.914Z'
)
const FigmaLogo = createBrandLogo(
  'M15.852 8.981h-4.588V0h4.588c2.476 0 4.49 2.014 4.49 4.49s-2.014 4.491-4.49 4.491zM12.735 7.51h3.117c1.665 0 3.019-1.355 3.019-3.019s-1.355-3.019-3.019-3.019h-3.117V7.51zm0 1.471H8.148c-2.476 0-4.49-2.014-4.49-4.49S5.672 0 8.148 0h4.588v8.981zm-4.587-7.51c-1.665 0-3.019 1.355-3.019 3.019s1.354 3.02 3.019 3.02h3.117V1.471H8.148zm4.587 15.019H8.148c-2.476 0-4.49-2.014-4.49-4.49s2.014-4.49 4.49-4.49h4.588v8.98zM8.148 8.981c-1.665 0-3.019 1.355-3.019 3.019s1.355 3.019 3.019 3.019h3.117V8.981H8.148zM8.172 24c-2.489 0-4.515-2.014-4.515-4.49s2.014-4.49 4.49-4.49h4.588v4.441c0 2.503-2.047 4.539-4.563 4.539zm-.024-7.51a3.023 3.023 0 0 0-3.019 3.019c0 1.665 1.365 3.019 3.044 3.019 1.705 0 3.093-1.376 3.093-3.068v-2.97H8.148zm7.704 0h-.098c-2.476 0-4.49-2.014-4.49-4.49s2.014-4.49 4.49-4.49h.098c2.476 0 4.49 2.014 4.49 4.49s-2.014 4.49-4.49 4.49zm-.097-7.509c-1.665 0-3.019 1.355-3.019 3.019s1.355 3.019 3.019 3.019h.098c1.665 0 3.019-1.355 3.019-3.019s-1.355-3.019-3.019-3.019h-.098z'
)
const CanvaLogo = createBrandLogo(
  'M12 0C5.373 0 0 5.373 0 12s5.373 12 12 12 12-5.373 12-12S18.627 0 12 0zM6.962 7.68c.754 0 1.337.549 1.405 1.2.069.583-.171 1.097-.822 1.406-.343.171-.48.172-.549.069-.034-.069 0-.137.069-.206.617-.514.617-.926.548-1.508-.034-.378-.308-.618-.583-.618-1.2 0-2.914 2.674-2.674 4.629.103.754.549 1.646 1.509 1.646.308 0 .65-.103.96-.24.5-.264.799-.47 1.097-.8-.073-.885.704-2.046 1.851-2.046.515 0 .926.205.96.583.068.514-.377.582-.514.582s-.378-.034-.378-.17c-.034-.138.309-.07.275-.378-.035-.206-.24-.274-.446-.274-.72 0-1.131.994-1.029 1.611.035.275.172.549.447.549.205 0 .514-.31.617-.755.068-.308.343-.514.583-.514.102 0 .17.034.205.171v.138c-.034.137-.137.548-.102.651 0 .069.034.171.17.171.092 0 .436-.18.777-.459.117-.59.253-1.298.253-1.357.034-.24.137-.48.617-.48.103 0 .171.034.205.171v.138l-.136.617c.445-.583 1.097-.994 1.508-.994.172 0 .309.102.309.274 0 .103 0 .274-.069.446-.137.377-.309.96-.412 1.474 0 .137.035.274.207.274.171 0 .685-.206 1.096-.754l.007-.004c-.002-.068-.007-.134-.007-.202 0-.411.035-.754.104-.994.068-.274.411-.514.617-.514.103 0 .205.069.205.171 0 .035 0 .103-.034.137-.137.446-.24.857-.24 1.269 0 .24.034.582.102.788 0 .034.035.069.07.069.068 0 .548-.445.89-1.028-.308-.206-.48-.549-.48-.96 0-.72.446-1.097.858-1.097.343 0 .617.24.617.72 0 .308-.103.65-.274.96h.102a.77.77 0 0 0 .584-.24.293.293 0 0 1 .134-.117c.335-.425.83-.74 1.41-.74.48 0 .924.205.959.582.068.515-.378.618-.515.618l-.002-.002c-.138 0-.377-.035-.377-.172 0-.137.309-.068.274-.376-.034-.206-.24-.275-.446-.275-.686 0-1.13.891-1.028 1.611.034.275.171.583.445.583.206 0 .515-.308.652-.754.068-.274.343-.514.583-.514.103 0 .17.034.205.171 0 .069 0 .206-.137.652-.17.308-.171.48-.137.617.034.274.171.48.309.583.034.034.068.102.068.102 0 .069-.034.138-.137.138-.034 0-.068 0-.103-.035-.514-.205-.72-.548-.789-.891-.205.24-.445.377-.72.377-.445 0-.89-.411-.96-.926a1.609 1.609 0 0 1 .075-.649c-.203.13-.422.203-.623.203h-.17c-.447.652-.927 1.098-1.27 1.303a.896.896 0 0 1-.377.104c-.068 0-.171-.035-.205-.104-.095-.152-.156-.392-.193-.667-.481.527-1.145.805-1.453.805-.343 0-.548-.206-.582-.55v-.376c.102-.754.377-1.2.377-1.337a.074.074 0 0 0-.069-.07c-.24 0-1.028.824-1.166 1.373l-.103.445c-.068.309-.377.515-.582.515-.103 0-.172-.035-.206-.172v-.137l.046-.233c-.435.31-.87.508-1.075.508-.308 0-.48-.172-.514-.412-.206.274-.445.412-.754.412-.352 0-.696-.24-.862-.593-.244.275-.523.553-.852.764-.48.309-1.028.549-1.68.549-.582 0-1.097-.309-1.371-.583-.412-.377-.651-.96-.686-1.509-.205-1.68.823-3.84 2.4-4.8.378-.205.755-.343 1.132-.343zm9.77 3.291c-.104 0-.172.172-.172.343 0 .274.137.583.309.755a1.74 1.74 0 0 0 .102-.583c0-.343-.137-.515-.24-.515z'
)
const HuggingFaceLogo = createBrandLogo(
  'M12.025 1.13c-5.77 0-10.449 4.647-10.449 10.378 0 1.112.178 2.181.503 3.185.064-.222.203-.444.416-.577a.96.96 0 0 1 .524-.15c.293 0 .584.124.84.284.278.173.48.408.71.694.226.282.458.611.684.951v-.014c.017-.324.106-.622.264-.874s.403-.487.762-.543c.3-.047.596.06.787.203s.31.313.4.467c.15.257.212.468.233.542.01.026.653 1.552 1.657 2.54.616.605 1.01 1.223 1.082 1.912.055.537-.096 1.059-.38 1.572.637.121 1.294.187 1.967.187.657 0 1.298-.063 1.921-.178-.287-.517-.44-1.041-.384-1.581.07-.69.465-1.307 1.081-1.913 1.004-.987 1.647-2.513 1.657-2.539.021-.074.083-.285.233-.542.09-.154.208-.323.4-.467a1.08 1.08 0 0 1 .787-.203c.359.056.604.29.762.543s.247.55.265.874v.015c.225-.34.457-.67.683-.952.23-.286.432-.52.71-.694.257-.16.547-.284.84-.285a.97.97 0 0 1 .524.151c.228.143.373.388.43.625l.006.04a10.3 10.3 0 0 0 .534-3.273c0-5.731-4.678-10.378-10.449-10.378M8.327 6.583a1.5 1.5 0 0 1 .713.174 1.487 1.487 0 0 1 .617 2.013c-.183.343-.762-.214-1.102-.094-.38.134-.532.914-.917.71a1.487 1.487 0 0 1 .69-2.803m7.486 0a1.487 1.487 0 0 1 .689 2.803c-.385.204-.536-.576-.916-.71-.34-.12-.92.437-1.103.094a1.487 1.487 0 0 1 .617-2.013 1.5 1.5 0 0 1 .713-.174m-10.68 1.55a.96.96 0 1 1 0 1.921.96.96 0 0 1 0-1.92m13.838 0a.96.96 0 1 1 0 1.92.96.96 0 0 1 0-1.92M8.489 11.458c.588.01 1.965 1.157 3.572 1.164 1.607-.007 2.984-1.155 3.572-1.164.196-.003.305.12.305.454 0 .886-.424 2.328-1.563 3.202-.22-.756-1.396-1.366-1.63-1.32q-.011.001-.02.006l-.044.026-.01.008-.03.024q-.018.017-.035.036l-.032.04a1 1 0 0 0-.058.09l-.014.025q-.049.088-.11.19a1 1 0 0 1-.083.116 1.2 1.2 0 0 1-.173.18q-.035.029-.075.058a1.3 1.3 0 0 1-.251-.243 1 1 0 0 1-.076-.107c-.124-.193-.177-.363-.337-.444-.034-.016-.104-.008-.2.022q-.094.03-.216.087-.06.028-.125.063l-.13.074q-.067.04-.136.086a3 3 0 0 0-.135.096 3 3 0 0 0-.26.219 2 2 0 0 0-.12.121 2 2 0 0 0-.106.128l-.002.002a2 2 0 0 0-.09.132l-.001.001a1.2 1.2 0 0 0-.105.212q-.013.036-.024.073c-1.139-.875-1.563-2.317-1.563-3.203 0-.334.109-.457.305-.454m.836 10.354c.824-1.19.766-2.082-.365-3.194-1.13-1.112-1.789-2.738-1.789-2.738s-.246-.945-.806-.858-.97 1.499.202 2.362c1.173.864-.233 1.45-.685.64-.45-.812-1.683-2.896-2.322-3.295s-1.089-.175-.938.647 2.822 2.813 2.562 3.244-1.176-.506-1.176-.506-2.866-2.567-3.49-1.898.473 1.23 2.037 2.16c1.564.932 1.686 1.178 1.464 1.53s-3.675-2.511-4-1.297c-.323 1.214 3.524 1.567 3.287 2.405-.238.839-2.71-1.587-3.216-.642-.506.946 3.49 2.056 3.522 2.064 1.29.33 4.568 1.028 5.713-.624m5.349 0c-.824-1.19-.766-2.082.365-3.194 1.13-1.112 1.789-2.738 1.789-2.738s.246-.945.806-.858.97 1.499-.202 2.362c-1.173.864.233 1.45.685.64.451-.812 1.683-2.896 2.322-3.295s1.089-.175.938.647-2.822 2.813-2.562 3.244 1.176-.506 1.176-.506 2.866-2.567 3.49-1.898-.473 1.23-2.037 2.16c-1.564.932-1.686 1.178-1.464 1.53s3.675-2.511 4-1.297c.323 1.214-3.524 1.567-3.287 2.405.238.839 2.71-1.587 3.216-.642.506.946-3.49 2.056-3.522 2.064-1.29.33-4.568 1.028-5.713-.624'
)
const Context7Logo = createBrandLogo(
  'M13.8027 0C11.193 0 8.583.9952 6.5918 2.9863c-3.9823 3.9823-3.9823 10.4396 0 14.4219 1.9911 1.9911 5.2198 1.9911 7.211 0 1.991-1.9911 1.991-5.2198 0-7.211L12 12c.9956.9956.9956 2.6098 0 3.6055-.9956.9955-2.6099.9955-3.6055 0-2.9866-2.9868-2.9866-7.8297 0-10.8164 2.9868-2.9868 7.8297-2.9868 10.8164 0l1.8028-1.8028C19.0225.9952 16.4125 0 13.8027 0zM12 12c-.9956-.9956-.9956-2.6098 0-3.6055.9956-.9955 2.6098-.9955 3.6055 0 2.9867 2.9868 2.9867 7.8297 0 10.8164-2.9867 2.9868-7.8297 2.9868-10.8164 0l-1.8028 1.8028c3.9823 3.9822 10.4396 3.9822 14.4219 0 3.9823-3.9824 3.9823-10.4396 0-14.4219-.9956-.9956-2.3006-1.4922-3.6055-1.4922-1.3048 0-2.6099.4966-3.6054 1.4922-1.9912 1.9912-1.9912 5.2198 0 7.211z'
)
const SlackLogo = createBrandLogo(
  'M5.042 15.165a2.528 2.528 0 0 1-2.52 2.523A2.528 2.528 0 0 1 0 15.165a2.527 2.527 0 0 1 2.522-2.52h2.52v2.52zM6.313 15.165a2.527 2.527 0 0 1 2.521-2.52 2.527 2.527 0 0 1 2.521 2.52v6.313A2.528 2.528 0 0 1 8.834 24a2.528 2.528 0 0 1-2.521-2.522v-6.313zM8.834 5.042a2.528 2.528 0 0 1-2.521-2.52A2.528 2.528 0 0 1 8.834 0a2.528 2.528 0 0 1 2.521 2.522v2.52H8.834zM8.834 6.313a2.528 2.528 0 0 1 2.521 2.521 2.528 2.528 0 0 1-2.521 2.521H2.522A2.528 2.528 0 0 1 0 8.834a2.528 2.528 0 0 1 2.522-2.521h6.312zM18.956 8.834a2.528 2.528 0 0 1 2.522-2.521A2.528 2.528 0 0 1 24 8.834a2.528 2.528 0 0 1-2.522 2.521h-2.522V8.834zM17.688 8.834a2.528 2.528 0 0 1-2.523 2.521 2.527 2.527 0 0 1-2.52-2.521V2.522A2.527 2.527 0 0 1 15.165 0a2.528 2.528 0 0 1 2.523 2.522v6.312zM15.165 18.956a2.528 2.528 0 0 1 2.523 2.522A2.528 2.528 0 0 1 15.165 24a2.527 2.527 0 0 1-2.52-2.522v-2.522h2.52zM15.165 17.688a2.527 2.527 0 0 1-2.52-2.523 2.526 2.526 0 0 1 2.52-2.52h6.313A2.527 2.527 0 0 1 24 15.165a2.528 2.528 0 0 1-2.522 2.523h-6.313z'
)

const logoIcons: Record<TemplateLogo, ComponentType<SVGProps<SVGSVGElement>>> = {
  notion: NotionLogo,
  shopify: ShopifyLogo,
  github: GitHubLogo,
  linear: LinearLogo,
  stripe: StripeLogo,
  vercel: VercelLogo,
  supabase: SupabaseLogo,
  cloudflare: CloudflareLogo,
  atlassian: AtlassianLogo,
  postman: PostmanLogo,
  sentry: SentryLogo,
  microsoft: MicrosoftLogo,
  firebase: FirebaseLogo,
  mongodb: MongoDbLogo,
  neon: NeonLogo,
  figma: FigmaLogo,
  canva: CanvaLogo,
  huggingface: HuggingFaceLogo,
  context7: Context7Logo,
  slack: SlackLogo,
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
  {
    id: 'atlassian-rovo',
    name: 'Atlassian Rovo',
    description: 'Search and manage Jira issues, Confluence pages, and Atlassian work.',
    category: 'productivity',
    popular: true,
    logo: 'atlassian',
    keywords: ['jira', 'confluence', 'issues', 'wiki', 'rovo'],
    values: {
      name: 'Atlassian Rovo',
      description: 'Search and manage Jira issues, Confluence pages, and Atlassian work.',
      transport: 'http',
      httpUrl: 'https://mcp.atlassian.com/v1/mcp/authv2',
      authType: 'auto',
    },
  },
  {
    id: 'postman',
    name: 'Postman',
    description: 'Manage API collections, workspaces, specifications, mocks, and monitors.',
    category: 'development',
    popular: true,
    logo: 'postman',
    keywords: ['api', 'collections', 'workspaces', 'mocks', 'monitors'],
    values: {
      name: 'Postman',
      description:
        'Manage Postman API collections, workspaces, specifications, mocks, and monitors.',
      transport: 'http',
      httpUrl: 'https://mcp.postman.com/minimal',
      authType: 'auto',
    },
  },
  {
    id: 'sentry',
    name: 'Sentry',
    description: 'Investigate application errors, issues, traces, and performance data.',
    category: 'development',
    popular: true,
    logo: 'sentry',
    keywords: ['errors', 'monitoring', 'traces', 'performance', 'observability'],
    values: {
      name: 'Sentry',
      description: 'Investigate Sentry application errors, issues, traces, and performance data.',
      transport: 'http',
      httpUrl: 'https://mcp.sentry.dev/mcp',
      authType: 'auto',
    },
  },
  {
    id: 'microsoft-learn',
    name: 'Microsoft Learn',
    description: 'Search current Microsoft documentation and retrieve official code samples.',
    category: 'development',
    popular: true,
    logo: 'microsoft',
    keywords: ['documentation', 'code samples', 'azure', 'dotnet', 'windows'],
    values: {
      name: 'Microsoft Learn',
      description: 'Search current Microsoft documentation and retrieve official code samples.',
      transport: 'http',
      httpUrl: 'https://learn.microsoft.com/api/mcp',
      authType: 'auto',
    },
  },
  {
    id: 'firebase',
    name: 'Firebase',
    description: 'Explore, configure, debug, and deploy authenticated Firebase projects.',
    category: 'infrastructure',
    popular: false,
    logo: 'firebase',
    keywords: ['google', 'backend', 'hosting', 'functions', 'firestore'],
    values: {
      name: 'Firebase',
      description: 'Explore, configure, debug, and deploy authenticated Firebase projects.',
      transport: 'npm',
      npmPackage: 'firebase-tools',
      npmVersion: '15.26.0',
      npmArgs: 'mcp',
      authType: 'auto',
    },
  },
  {
    id: 'mongodb',
    name: 'MongoDB',
    description: 'Explore schemas and query MongoDB or Atlas safely in read-only mode.',
    category: 'infrastructure',
    popular: false,
    logo: 'mongodb',
    keywords: ['database', 'atlas', 'query', 'schema', 'nosql'],
    values: {
      name: 'MongoDB',
      description: 'Explore schemas and query MongoDB or Atlas safely in read-only mode.',
      transport: 'npm',
      npmPackage: 'mongodb-mcp-server',
      npmVersion: '2.0.0',
      npmArgs: '--readOnly',
      npmEnv: [
        {
          id: 'template-mongodb-connection',
          name: 'MDB_MCP_CONNECTION_STRING',
          value: '',
          hasValue: false,
        },
      ],
      authType: 'auto',
    },
  },
  {
    id: 'neon',
    name: 'Neon',
    description: 'Manage Neon Postgres projects, branches, databases, queries, and migrations.',
    category: 'infrastructure',
    popular: false,
    logo: 'neon',
    keywords: ['postgres', 'database', 'branches', 'sql', 'migrations'],
    values: {
      name: 'Neon',
      description: 'Manage Neon Postgres projects, branches, databases, queries, and migrations.',
      transport: 'http',
      httpUrl: 'https://mcp.neon.tech/mcp',
      authType: 'auto',
    },
  },
  {
    id: 'figma',
    name: 'Figma',
    description: 'Inspect design context and components with Figma-supported OAuth client access.',
    category: 'productivity',
    popular: true,
    logo: 'figma',
    keywords: ['design', 'components', 'ui', 'prototype', 'code'],
    values: {
      name: 'Figma',
      description: 'Inspect Figma design context and components for design-to-code workflows.',
      transport: 'http',
      httpUrl: 'https://mcp.figma.com/mcp',
      authType: 'auto',
    },
  },
  {
    id: 'canva',
    name: 'Canva',
    description: 'Create, edit, search, organize, and export Canva designs after client approval.',
    category: 'productivity',
    popular: false,
    logo: 'canva',
    keywords: ['design', 'graphics', 'export', 'creative', 'presentation'],
    values: {
      name: 'Canva',
      description: 'Create, edit, search, organize, and export Canva designs.',
      transport: 'http',
      httpUrl: 'https://mcp.canva.com/mcp',
      authType: 'auto',
    },
  },
  {
    id: 'hugging-face',
    name: 'Hugging Face',
    description: 'Search models, datasets, Spaces, papers, documentation, and jobs.',
    category: 'development',
    popular: false,
    logo: 'huggingface',
    keywords: ['ai', 'models', 'datasets', 'spaces', 'machine learning'],
    values: {
      name: 'Hugging Face',
      description: 'Search Hugging Face models, datasets, Spaces, papers, documentation, and jobs.',
      transport: 'http',
      httpUrl: 'https://huggingface.co/mcp',
      authType: 'auto',
    },
  },
  {
    id: 'context7',
    name: 'Context7',
    description: 'Retrieve current, version-specific library documentation and code examples.',
    category: 'development',
    popular: false,
    logo: 'context7',
    keywords: ['documentation', 'libraries', 'code examples', 'upstash', 'context'],
    values: {
      name: 'Context7',
      description: 'Retrieve current, version-specific library documentation and code examples.',
      transport: 'http',
      httpUrl: 'https://mcp.context7.com/mcp',
      authType: 'bearer',
    },
  },
  {
    id: 'slack',
    name: 'Slack',
    description: 'Search and manage Slack content with a registered, admin-approved Slack app.',
    category: 'productivity',
    popular: false,
    logo: 'slack',
    keywords: ['messages', 'channels', 'workspace', 'canvases', 'collaboration'],
    values: {
      name: 'Slack',
      description: 'Search messages and manage Slack content using a registered Slack app.',
      transport: 'http',
      httpUrl: 'https://mcp.slack.com/mcp',
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
              <Card key={template.id} padding={5}>
                <VStack gap={4} hAlign="stretch">
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

                  <Text type="body" color="secondary" maxLines={3}>
                    {template.description}
                  </Text>

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
