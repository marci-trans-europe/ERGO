export type ProductKind = 'product' | 'marketing-fund' | 'rebate' | 'service'

export type ProductClassification = {
  category: string
  manufacturer: string
  kind: ProductKind
}

export const categoryOrder = [
  'AI és analitika',
  'Kommunikáció',
  'Üzletmenet-folytonosság',
  'Humán erőforrás',
  'Infrastruktúra',
  'Integrációk',
  'IT-üzemeltetés',
  'Hálózat',
  'Produktivitás & irodai alkalmazások',
  'Értékesítés és marketing',
  'Biztonság',
  'Fejlesztőeszközök és DevOps',
  'Design és kreatív',
  'Média és szórakoztatás',
  'Mérnöki és CAD',
  'Oktatástechnológia',
  'Hardver és periféria',
  'Marketing fund',
  'Szakmai szolgáltatások és díjak',
  'Szakmai üzleti alkalmazások',
  'Általános szoftverlicencek és kiegészítők',
] as const

const rules: Array<[string, RegExp]> = [
  ['Biztonság', /\b(security|antivirus|antimalware|endpoint protection|application control|device control|file protection|firewall|fortinet|crowdstrike|sentinelone|sophos|cyber|zero trust|vulnerability|pentest|bitdefender|kaspersky|securecrt|erasure|certus|encryption|proofpoint|mfa|siem|password manager|1password|e[ds]r|xdr|checkmarx|nowsecure|usercentrics|safetica|accessdata ftk|ftk|cerberus|forensic toolkit|face sdk|facesdk|face matcher|face extractor|currentware|titania|nipper|qualys|burp suite|spamtitan|veriato|drivelock|bitlocker|xstream protection|defender management|sid changer)\b/i],
  ['Hardver és periféria', /\b(wacom|cintiq|intuos|movink|bamboo ink|pantone|diskashur|lcd signature tablet|usb drive|pen tablet|colorreader|spyderx? pro|physical appliance)\b/i],
  ['Üzletmenet-folytonosság', /\b(backup|veeam|acronis|arcserve|macrium reflect|disaster recovery|email archiving|saas backup|data recovery|data protection|replication|restore point)\b/i],
  ['Oktatástechnológia', /\b(e-learning|learning management|classroom software|iskolai alkalmazás|oktatási platform|ispring|kodekloud|mozabook|adobe captivate|articulate 360)\b/i],
  ['Média és szórakoztatás', /\b(adobe premiere|premiere pro|after effects|davinci resolve|final cut|cinema 4d|video editing|film production|broadcast|vfx|red giant|pinnacle studio|boris fx|vyond|goanimate|sound forge|adobe audition|ableton|pro tools|cubase|logic pro|steinberg|music production|audio production)\b/i],
  ['AI és analitika', /\b(deepl|copilot|chatgpt|openai|machine learning|artificial intelligence|ai assistant|ai credits|business intelligence|power bi|tableau|data analytics|alteryx|originpro|minitab|eviews|matomo|maxqda|diadem|tabular editor|statistical software)\b/i],
  ['Fejlesztőeszközök és DevOps', /\b(jetbrains|intellij|pycharm|webstorm|phpstorm|rider|clion|rubymine|goland|datagrip|visual studio|github|gitlab|docker|devops|developer|development|source code|jenkins|jfrog|sentry|postman|bitbucket|sonarqube|sonatype|teamcity|jrebel|youtrack|atlassian jira|confluence|semaphore|toad for oracle|toad dba|toad edge|toad data modeler|sql prompt|dbvisualizer|dbforge|xmlspy|oxygen xml editor|altova|incredibuild|studio 3t|syncfusion|nutrient android sdk|enterprise architect|sparx systems|prolaborate|filemaker|installshield|dezign for databases|database design|powershell studio|coldfusion|sdk subscription)\b/i],
  ['Mérnöki és CAD', /\b(revit|archicad|sketchup|solidworks|solid edge|autocad|zwcad|bricscad|inventor|catia|ansys|civil 3d|tekla|bim|bluebeam|cad|cam|plm|engineering|fusion 360|powerfactory|stability analysis functions|electromagnetic transients|rhinoceros|navisworks|comsol|multiphysics|labview|cfd module|structural mechanics|eplan|agisoft metashape|3df zephyr|pv sol|cx-programmer)\b/i],
  ['Design és kreatív', /\b(creative cloud|adobe cc|adobe all apps|all apps edition|photoshop|illustrator|indesign|incopy|adobe xd|figma|coreldraw|corel academic|cdgs perpetual|lightroom|substance 3d|dreamweaver|adobe stock|sketch app|affinity|monotype|enfocus|pitstop pro|max[o]?n|envato|extensis|axure|lumion|v-ray|vray|3ds max|blender|twinmotion|enscape|capture one|quarkxpress|datacolor|daminion|chaos corona|autodesk maya|maya 202[0-9])\b/i],
  ['Infrastruktúra', /\b(vmware|virtualiz|vcenter|azure|aws|amazon web services|cloud server|windows server|red hat|rhel|linux enterprise|ubuntu pro|storage|nas|database server|sql server|hyper-v|citrix|parallels|vps)\b/i],
  ['IT-üzemeltetés', /\b(teamviewer|anydesk|ninjaone|remote desktop|remote access|moba[x]?term|royal ts|devolutions|rmm|monitoring|redgate sql monitor|nagios|lansweeper|pager\s?duty|intune|system center|help desk|service desk|manageengine|patch management|mdm|suremdm|ivanti|sharegate|saas management|device management|adminstudio|aida64|dataedo)\b/i],
  ['Hálózat', /\b(network|networking|router|switch|wifi|wi-fi|dns|sd-wan|vpn|unifi|ubiquiti)\b/i],
  ['Kommunikáció', /\b(teams phone|teams essentials|zoom|webex|voip|sip trunk|contact center|telecom|telekommunik|ucaas|ccaas|softphone|mailgun|messaging gateway|symprex mail signature|email signature)\b/i],
  ['Értékesítés és marketing', /\b(crm|salesforce|hubspot|mailchimp|marketing|semrush|social media|email campaign|sales acceleration|bitly|sitesearch)\b/i],
  ['Integrációk', /\b(zapier|make\.com|automation|automate|rpa|integration platform|workflow automation|power automate|globalscape|enhanced file transfer|zappsys|ssis powerpack|kepserverex|opc manufacturing)\b/i],
  ['Humán erőforrás', /\b(payroll|human resources|hr management|personnel management|bérszámfejtés|munkaügyi rendszer)\b/i],
  ['Produktivitás & irodai alkalmazások', /\b(acrobat|office|microsoft 365|word|excel|powerpoint|pdf|adobe sign|docusign|think-cell|abbyy|finereader|fine reader|notion|asana|monday\.com|microsoft project|miro|mindmanager|winzip|help\+manual|framemaker)\b/i],
  ['Szakmai üzleti alkalmazások', /\b(erp|business application|industry application|agleader sms|spatial management|gis|crosscap|inventory management|accounting software|business suite)\b/i],
]

const brands: Array<[string, RegExp]> = [
  ['Adobe', /\b(adobe|creative cloud|acrobat|photoshop|illustrator|indesign|lightroom|premiere pro|after effects|framemaker|coldfusion)\b/i],
  ['Wacom', /\b(wacom|cintiq|intuos|movink|bamboo ink)\b/i],
  ['Pantone', /\bpantone\b/i],
  ['Autodesk', /\b(autodesk|autocad|revit|inventor|navisworks|fusion 360|3ds max|maya 202[0-9])\b/i],
  ['Microsoft', /\b(microsoft|windows server|power bi|copilot|azure|visual studio)\b/i],
  ['JetBrains', /\b(jetbrains|intellij|pycharm|webstorm|phpstorm|rubymine|goland|datagrip|teamcity|youtrack)\b/i],
  ['DeepL', /\bdeepl\b/i],
  ['AnyDesk', /\banydesk\b/i],
  ['VMware', /\b(vmware|vsphere|vcenter)\b/i],
  ['Corel', /\b(corel|coreldraw|cdgs)\b/i],
  ['Figma', /\bfigma\b/i],
  ['Veeam', /\bveeam\b/i],
  ['Acronis', /\bacronis\b/i],
  ['TeamViewer', /\bteamviewer\b/i],
  ['Bitdefender', /\bbitdefender\b/i],
  ['Sophos', /\bsophos\b/i],
  ['CrowdStrike', /\bcrowdstrike\b/i],
  ['Fortinet', /\bfortinet\b/i],
  ['Docker', /\bdocker\b/i],
  ['JFrog', /\bjfrog\b/i],
  ['GitHub', /\bgithub\b/i],
  ['Atlassian', /\b(atlassian|jira|confluence)\b/i],
  ['ABBYY', /\b(abbyy|finereader|fine reader)\b/i],
  ['Bricsys', /\bbricscad\b/i],
  ['SketchUp', /\bsketchup\b/i],
  ['SOLIDWORKS', /\bsolidworks\b/i],
  ['EPLAN', /\beplan\b/i],
  ['Articulate', /\barticulate\b/i],
  ['iSpring', /\bispring\b/i],
  ['ManageEngine', /\bmanageengine\b/i],
  ['PagerDuty', /\bpager\s?duty\b/i],
  ['Zoom', /\bzoom\b/i],
  ['Cisco', /\b(webex|cisco)\b/i],
  ['Salesforce', /\bsalesforce\b/i],
  ['HubSpot', /\bhubspot\b/i],
  ['Bitly', /\bbitly\b/i],
  ['Certus', /\bcertus\b/i],
  ['Quark', /\bquarkxpress\b/i],
  ['Chaos', /\b(chaos|v-ray|vray|corona solo)\b/i],
]

const manufacturerFor = (name: string): string => {
  const known = brands.find(([, pattern]) => pattern.test(name))
  if (known) return known[0]
  const first = name.match(/[\p{L}][\p{L}\d+.-]*/u)?.[0]
  if (!first || /^(see|fy\d+|s|license|licence|subscription|software|service|mdf)$/i.test(first)) {
    return 'Nem azonosított gyártó'
  }
  return first
}

export const classifyProduct = (
  itemNumber: string,
  itemName: string,
): ProductClassification => {
  const name = itemName.trim()
  const code = itemNumber.trim()
  const manufacturer = manufacturerFor(name)

  if (/(?:^|[^a-z0-9])mdf(?:$|[^a-z0-9])/i.test(code) || /\bmdf\b/i.test(name) || /\bmovink product launch\b/i.test(name)) {
    return { category: 'Marketing fund', manufacturer, kind: 'marketing-fund' }
  }
  if (/\b(commission cost|reseller commission|rebate|jutalék)\b/i.test(name)) {
    return {
      category: /\bautodesk\b/i.test(name) || /^AUTODESK_COSMM_COST$/i.test(code)
        ? 'Mérnöki és CAD'
        : 'Szakmai szolgáltatások és díjak',
      manufacturer,
      kind: 'rebate',
    }
  }
  if (/\b(on-site professional services|professional service|consulting service|tanácsadási szolgáltatás|helyszíni szolgáltatás)\b/i.test(name)) {
    return { category: 'Szakmai szolgáltatások és díjak', manufacturer, kind: 'service' }
  }
  if (/^pf_/i.test(code)) {
    return { category: 'Mérnöki és CAD', manufacturer, kind: 'product' }
  }
  const category = rules.find(([, pattern]) => pattern.test(name))?.[0]
    ?? (/\b(licen[cs]e|subscription|renewal|maintenance|előfizetés|licenc|licenckövetés)\b/i.test(name)
      ? 'Általános szoftverlicencek és kiegészítők'
      : 'Szakmai üzleti alkalmazások')
  return { category, manufacturer, kind: 'product' }
}
