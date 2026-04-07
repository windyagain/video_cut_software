# 2025-04-06 部署记录

## 项目概览

### 1. video_cut_software (Rust + Tauri)
**状态**: 已推送至GitHub (tag v0.2.0)

**今日新增功能**:
- `strip-audio` CLI命令：提取音频为WAV格式
- 圆角字幕背景宽度计算优化（CJK: 0.98, ASCII: 0.58, 空格: 0.33）
- `xPaddingScale` 参数控制水平留白
- `probe_video_size` 动态分辨率检测
- UI默认勾选圆角选项

**关键文件**:
- `crates/video_cli/src/main.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/index.html`

---

### 2. tool-web/orange (Vue + Spring Boot)
**部署地址**: http://101.34.207.228/tool-web/
**后端API**: http://101.34.207.228:81

#### 2.1 阿里云ASR功能修复

**问题1**: 请求格式错误
- **现象**: `InvalidParameter: input must contain file_urls`
- **原因**: 请求体缺少 `input` 包装层
- **修复**: 修改 `AliyunAudioBiz.buildAsrRequest()`
```java
// 正确格式
{
  "model": "fun-asr",
  "input": {
    "file_urls": ["http://..."],
    "language_hints": ["zh"]
  }
}
```

**问题2**: 缺少language_hints参数
- **现象**: 某些模型要求language_hints
- **修复**: 添加 `language_hints: ["zh"]` 参数
- **新增参数**: Controller支持 `language` 参数（默认zh）

**问题3**: 结尾句号去除
- **修复**: `stripEndingPunctuation()` 方法自动去除结尾"。"

#### 2.2 静态资源配置修复

**配置项** (`application-pro.yml`):
```yaml
file:
  upload:
    directory: /Users/pxy/Documents/uploads/

spring:
  web:
    resources:
      static-locations: /Users/pxy/Documents/uploads/
```

**Docker挂载** (docker-compose.yml):
```yaml
volumes:
  - ./uploads:/Users/pxy/Documents/uploads/
```

**关键**: 挂载路径必须与配置完全一致！

#### 2.3 Nginx配置

**静态文件服务** (`/home/lighthouse/apps/nginx/nginx.conf`):
```nginx
location /static {
   alias /usr/share/nginx/html/uploads;
   expires 1h;
   add_header Cache-Control "public, immutable";
}
```

**挂载**:
- `/home/lighthouse/apps/tool-back/uploads` → `/usr/share/nginx/html/uploads`

#### 2.4 前端配置

**API地址** (`vue-front-project/orange/src/config/global_config.js`):
```javascript
API_ORIGIN: 'http://101.34.207.228:81',
API_NGINX_ORIGIN: 'http://101.34.207.228',
HTML_PREVIEW_ORIGIN: 'http://101.34.207.228:81',
```

---

## 服务器目录结构

```
/home/lighthouse/
├── apps/
│   ├── tool-back/           # Java后端
│   │   ├── orange.jar       # 主程序
│   │   ├── docker-compose.yml
│   │   └── uploads/         # 上传文件目录
│   │       └── others/
│   │           └── quick_20s.audio.wav
│   ├── nginx/
│   │   ├── nginx.conf       # 主配置
│   │   └── html/
│   │       └── tool-web/    # 前端部署目录
│   └── nginx-compose/html/  # 前端构建临时目录
└── nginx-compose/           # 另一个nginx配置（未使用）
```

---

## Docker容器

| 容器名 | 镜像 | 端口 | 用途 |
|--------|------|------|------|
| tool-orange | openjdk:8-jdk-alpine | 81:81 | Java后端 |
| web-nginx | nginx:1.25.1 | 80:80 | 前端静态服务 |

---

## 验证命令

```bash
# 测试ASR接口
curl -s -X POST 'http://101.34.207.228:81/tool/audio_asr' \
  -F 'file=@/home/lighthouse/apps/tool-back/uploads/others/quick_20s.audio.wav' \
  -F 'dashscope_api_key=sk-xxx' \
  -F 'model=fun-asr' \
  -F 'language=zh'

# 测试静态文件访问
curl -I 'http://101.34.207.228/static/others/quick_20s.audio.wav'

# 查看容器日志
docker logs tool-orange --tail 50
```

---

## 下次部署注意事项

1. **前端构建前**: 确认 `global_config.js` 中API地址正确
2. **后端构建前**: 确认 `application-pro.yml` 中路径配置
3. **Docker**: 确认挂载路径与配置一致
4. **Nginx**: 静态文件配置检查 `/static` 路径

---

## Git提交记录

- `d07cc7f` fix: 阿里云ASR添加language_hints参数
- `0b88375` fix: 前端API地址改为远程服务器
- （后续提交）fix: 去除ASR结果结尾句号
