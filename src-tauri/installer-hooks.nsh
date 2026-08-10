; VNT GUI 安装/卸载钩子（tauri.conf.json bundle.windows.nsis.installerHooks）
;
; 数据文件（config.json / ftp_config.json / runtime_state.json / daemon.pid / vnt-daemon.log）
; 统一存放于应用安装目录（$INSTDIR）。
; 卸载流程：模板内置"删除应用数据"复选框 → un.ConfirmLeave 记录 $DeleteAppDataCheckboxState。
; - 勾选：删除安装目录全部内容（含配置/日志），实现"全部删除"
; - 未勾选：模板仅删除注册文件，数据残留于 $INSTDIR，实现"保留数据"
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    RmDir /r "$INSTDIR"
  ${EndIf}
!macroend
