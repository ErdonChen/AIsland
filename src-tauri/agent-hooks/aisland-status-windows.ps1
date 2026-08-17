[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('codex', 'hermes', 'workbuddy', 'claude')][string]$Agent,
    [Parameter(Mandatory = $true)][ValidateSet('windows')][string]$Environment,
    [Parameter(Mandatory = $true)][string]$NativeEvent,
    [Parameter(Mandatory = $true)][string]$OutputPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Fail-Input { param([string]$Code) [Console]::Error.WriteLine($Code); exit 1 }
function Get-NativeValue {
    param([object]$Object, [string]$Name)
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}
function Get-OptionalText {
    param([object]$Object, [string]$Name)
    $candidate = Get-NativeValue $Object $Name
    if ($candidate -isnot [string]) { return $null }
    $value = ([string]$candidate).Trim()
    if ($value.Length -eq 0) { return $null }
    $utf8 = [Text.Encoding]::UTF8
    $bytes = $utf8.GetBytes($value)
    $count = [Math]::Min($bytes.Length, 1024)
    while ($count -gt 0 -and $count -lt $bytes.Length -and (($bytes[$count] -band 0xC0) -eq 0x80)) { $count-- }
    return $utf8.GetString($bytes, 0, $count)
}
function Get-NormalizedStatus {
    param([object]$Payload, [string]$Id, [string]$Event)
    $timeout = ((Get-NativeValue $Payload 'failure_reason') -eq 'timeout') -or ((Get-NativeValue $Payload 'timeout') -eq $true)
    $extra = Get-NativeValue $Payload 'extra'
    if ($extra -is [pscustomobject] -and (Get-NativeValue $extra 'choice') -eq 'timeout') { $timeout = $true }
    $failed = ((Get-NativeValue $Payload 'success') -eq $false) -or ((Get-NativeValue $Payload 'failed') -eq $true)
    if ($Event -eq 'PermissionRequest' -or $Event -eq 'pre_approval_request') { return 'waiting' }
    if ($Event -in @('SessionStart', 'UserPromptSubmit', 'on_session_start', 'pre_llm_call')) { return 'running' }
    if ($Event -eq 'StopFailure') { if ($timeout) { return 'timeout' }; return 'failed' }
    if ($Event -eq 'Stop' -or $Event -eq 'post_llm_call') { return 'completed' }
    if ($Event -eq 'SessionEnd' -or $Event -eq 'on_session_end') { if ($timeout) { return 'timeout' }; if ($failed) { return 'failed' }; return 'idle' }
    if ($Event -eq 'post_approval_response') {
        if ($timeout) { return 'timeout' }; return 'running'
    }
    return 'running'
}
function Get-AgentReplyPreview {
    param([object]$Payload, [string]$Event)
    if ($Event -notin @('Stop', 'SubagentStop', 'post_llm_call')) { return $null }
    if ($Event -eq 'post_llm_call') {
        $extra = Get-NativeValue $Payload 'extra'
        if ($extra -is [pscustomobject]) {
            $assistantResponse = Get-OptionalText $extra 'assistant_response'
            if ($null -ne $assistantResponse) { return $assistantResponse }
        }
    }
    return Get-OptionalText $Payload 'last_assistant_message'
}
function Set-CurrentUserOnlyAcl {
    param([string]$Path)
    try {
        $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
        $acl = [Security.AccessControl.FileSecurity]::new()
        $acl.SetAccessRuleProtection($true, $false)
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($sid, [Security.AccessControl.FileSystemRights]::FullControl, 'Allow'))
        [IO.File]::SetAccessControl($Path, $acl)
        $actual = (Get-Item -LiteralPath $Path).GetAccessControl()
        $rules = @($actual.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
        $ruleSid = $rules[0].IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
        if (-not $actual.AreAccessRulesProtected -or $rules.Count -ne 1 -or $ruleSid -ne $sid.Value -or $rules[0].AccessControlType -ne 'Allow') { throw 'aclVerificationFailed' }
    } catch [PlatformNotSupportedException] { return }
}

try {
    $stage = 'readStdin'
    $stream = [Console]::OpenStandardInput()
    $buffer = New-Object byte[] 8192
    $memory = [IO.MemoryStream]::new()
    while (($read = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
        if ($memory.Length + $read -gt 1048576) { Fail-Input 'payloadTooLarge' }
        $memory.Write($buffer, 0, $read)
    }
    $stage = 'decodeUtf8'
    $raw = [Text.UTF8Encoding]::new($false, $true).GetString($memory.ToArray())
    $stage = 'parseJson'
    $native = $raw | ConvertFrom-Json -ErrorAction Stop
    if ($native -isnot [pscustomobject]) { Fail-Input 'invalidPayload' }
    $stage = 'normalize'
    $extra = Get-NativeValue $native 'extra'
    $taskId = Get-NativeValue $native 'task_id'
    if ($null -eq $taskId -and $extra -is [pscustomobject]) { $taskId = Get-NativeValue $extra 'task_id' }
    if ($null -eq $taskId) { $sessionId = Get-NativeValue $native 'session_id'; if ($sessionId -is [string]) { $taskId = "session:$sessionId" } }
    $nativeEventId = Get-NativeValue $native 'event_id'
    if ($taskId -isnot [string] -or [Text.Encoding]::UTF8.GetByteCount($taskId) -eq 0 -or [Text.Encoding]::UTF8.GetByteCount($taskId) -gt 256) { Fail-Input 'invalidIdentifier' }
    $sequenceValue = Get-NativeValue $native 'sequence'
    $sequence = if ($sequenceValue -is [long] -or $sequenceValue -is [int]) { [uint64]$sequenceValue } else { $null }
    $nativeOccurredAt = Get-NativeValue $native 'occurred_at'
    $sourceOccurredAt = if ($nativeOccurredAt -is [long] -or $nativeOccurredAt -is [int]) { [string][int64]$nativeOccurredAt } else { 'missing-occurred-at' }
    $occurredAt = if ($nativeOccurredAt -is [long] -or $nativeOccurredAt -is [int]) { [int64]$nativeOccurredAt } else { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
    if (($nativeEventId -isnot [string] -or [Text.Encoding]::UTF8.GetByteCount($nativeEventId) -eq 0 -or [Text.Encoding]::UTF8.GetByteCount($nativeEventId) -gt 128) -and $extra -is [pscustomobject]) {
        $turnId = Get-NativeValue $extra 'turn_id'
        if ($turnId -is [string] -and [Text.Encoding]::UTF8.GetByteCount($turnId) -gt 0 -and [Text.Encoding]::UTF8.GetByteCount($turnId) -le 96) { $nativeEventId = "$NativeEvent`n$turnId" }
    }
    if ($nativeEventId -isnot [string] -or [Text.Encoding]::UTF8.GetByteCount($nativeEventId) -eq 0 -or [Text.Encoding]::UTF8.GetByteCount($nativeEventId) -gt 128) {
        $nativeEventId = "$Agent`n$Environment`n$taskId`n$NativeEvent`n$sequence`n$sourceOccurredAt"
    }
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try { $digest = $sha256.ComputeHash([Text.Encoding]::UTF8.GetBytes($nativeEventId)) } finally { $sha256.Dispose() }
    $eventId = "aisland-$Agent-$Environment-" + ([BitConverter]::ToString($digest).Replace('-', '')).ToLowerInvariant()
    $status = Get-NormalizedStatus $native $Agent $NativeEvent
    $wire = [ordered]@{
        schema_version = 1; event_id = $eventId; agent = $Agent; environment = $Environment; task_id = $taskId; status = $status
        occurred_at = $occurredAt
        sequence = $sequence
        task_title = Get-OptionalText $native 'task_title'; project = Get-OptionalText $native 'project'; path = Get-OptionalText $native 'path'
    }
    $replyPreview = Get-AgentReplyPreview $native $NativeEvent
    if ($null -ne $replyPreview) { $wire['message'] = "aisland-agent-reply-v1:$replyPreview" }
    $stage = 'serialize'
    $json = $wire | ConvertTo-Json -Compress -Depth 3
    $stage = 'prepareTarget'
    $target = [IO.Path]::GetFullPath($OutputPath)
    $directory = [IO.Path]::GetDirectoryName($target)
    [IO.Directory]::CreateDirectory($directory) | Out-Null
    $temporary = "$target.$([Guid]::NewGuid().ToString('N')).tmp"
    $backup = "$temporary.backup"
    try {
        $stage = 'writeTemporary'
        $file = [IO.FileStream]::new($temporary, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $bytes = [Text.UTF8Encoding]::new($false).GetBytes($json); $file.Write($bytes, 0, $bytes.Length); $file.Flush($true) } finally { $file.Dispose() }
        $stage = 'secureTemporary'
        Set-CurrentUserOnlyAcl $temporary
        $stage = 'publish'
        if ([IO.File]::Exists($target)) { [IO.File]::Replace($temporary, $target, $backup); [IO.File]::Delete($backup) } else { [IO.File]::Move($temporary, $target) }
        $stage = 'securePublished'
        Set-CurrentUserOnlyAcl $target
    } finally { if ([IO.File]::Exists($temporary)) { [IO.File]::Delete($temporary) }; if ([IO.File]::Exists($backup)) { [IO.File]::Delete($backup) } }
} catch {
    [Console]::Error.WriteLine("stage=$stage kind=$($_.Exception.GetType().Name) invalidPayload")
    exit 1
}
