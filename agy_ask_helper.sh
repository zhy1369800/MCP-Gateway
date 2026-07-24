# 获取当前终端设备的唯一标识作为会话隔离键，解决 su - 导致 TMUX 变量丢失的问题
_tty_id=$(tty 2>/dev/null | sed 's|/dev/||; s|/|_|g')
if [[ -z "$_tty_id" || "$_tty_id" == *"not a tty"* ]]; then
    _tty_id="default"
fi

export AGY_LOG_FILE="/tmp/agy_conv_${_tty_id}.log"
export AGY_SID_STORE="/tmp/agy_sid_${_tty_id}.env"
unset _tty_id

# 记录最外层 Shell 的 PID（用于避免子 Shell 退出时误删日志）
if [[ -z "$AGY_OWNER_PID" ]]; then
    export AGY_OWNER_PID="$$"
fi

# 启动时若存在持久化文件，自动恢复会话 ID
if [[ -f "$AGY_SID_STORE" ]]; then
    source "$AGY_SID_STORE" 2>/dev/null
fi

# 注册退出时的清理机制（仅当创建该环境的 Owner Shell 退出时才执行删除）
trap 'if [[ "$$" == "$AGY_OWNER_PID" ]]; then rm -f "$AGY_LOG_FILE" "$AGY_SID_STORE"; fi' EXIT

# agy_ask: 自动维护会话上下文的提问函数
agy_ask() {
    local prompt_msg=""
    if [[ "$1" == "-p" || "$1" == "--prompt" ]]; then
        shift
        prompt_msg="$*"
    else
        prompt_msg="$*"
    fi

    if [[ -z "$prompt_msg" ]]; then
        echo "使用方法: agy_ask \"你的问题\"" >&2
        return 1
    fi

    # 兜底：如果变量 agy_sid 没有值，再次尝试从持久化文件读取
    if [[ -z "$agy_sid" && -f "$AGY_SID_STORE" ]]; then
        source "$AGY_SID_STORE" 2>/dev/null
    fi

    # 如果依然没有值，进行初始化
    if [[ -z "$agy_sid" ]]; then
        echo "初始化 agy 会话上下文中..." >&2
        command agy -p "hi" --log-file "$AGY_LOG_FILE" >/dev/null 2>&1
        
        # 从日志文件中获取 Created conversation ID
        agy_sid=$(grep -oE "Created conversation [a-f0-9-]+" "$AGY_LOG_FILE" | tail -n 1 | awk '{print $3}')
        
        if [[ -n "$agy_sid" ]]; then
            export agy_sid
            # 持久化到本地临时文件，以便子 Shell / Shell 重启时复用
            echo "export agy_sid='$agy_sid'" > "$AGY_SID_STORE"
            echo "✅ 成功绑定 agy 会话 ID: $agy_sid" >&2
        else
            echo "❌ 初始化会话 ID 失败，请检查 agy 服务状态" >&2
            return 1
        fi
    fi

    # 使用绑定好的会话 ID 进行提问
    command agy --conversation "$agy_sid" -p "$prompt_msg"
}
export -f agy_ask

# 别名映射，方便输入
alias agy-ask='agy_ask'
