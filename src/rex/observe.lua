-- Return only declared status fields. Never read screens, prompts, cwd or msg.
local result, err = rex.session.list_blocks{}
if not result then error(err) end
local entries = {}
for _, block in ipairs(result.blocks) do
  if block.creator_name == "com.superlogical.terminal" and block.placement == "placed" then
    local status, status_err = rex.block.call("com.superlogical.terminal", "program_status", {block_id=block.block_id})
    if not status then error(status_err) end
    for _, record in ipairs(status.records) do
      entries[#entries+1] = {block_id=block.block_id, record_id=record.id or "", app=record.effective_app, state=record.state, name=record.title}
    end
  end
end
return {version=1, entries=entries}
