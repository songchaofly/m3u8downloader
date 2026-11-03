import { test,expect } from '@playwright/test';

test('desktop workspace renders and new task form validates without runtime errors',async({page})=>{
  const errors:string[]=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('/');await expect(page.getByRole('heading',{name:'下载工作台'})).toBeVisible();
  await page.getByRole('button',{name:'新建任务',exact:true}).click();await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByRole('button',{name:'开始下载',exact:true})).toBeDisabled();
  await page.getByPlaceholder('https://example.com/video/index.m3u8').fill('https://example.com/test.m3u8');
  await expect(page.getByRole('button',{name:'开始下载',exact:true})).toBeEnabled();
  await page.getByText('请求、代理与解密',{exact:true}).click();await expect(page.getByText('请求头 JSON（Cookie、Referer 等）',{exact:true})).toBeVisible();
  expect(errors).toEqual([]);await page.screenshot({path:'test-results/new-download.png',fullPage:true});
});

test('empty workspace has clear actions and supports dark theme',async({page})=>{
  await page.goto('/');await expect(page.getByRole('heading',{name:'准备好保存你的第一段媒体'})).toBeVisible();
  await page.getByTitle('切换主题').click();await expect(page.locator('html')).toHaveAttribute('data-theme','dark');
  await page.screenshot({path:'test-results/workspace-dark.png',fullPage:true});
});
