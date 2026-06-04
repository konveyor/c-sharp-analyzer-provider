using System;
using System.Collections.Generic;
using System.Web;
using System.Web.UI;
using System.Web.UI.WebControls;
using LegacyApp.DataAccess;
using LegacyApp.Models;
using log4net;

namespace LegacyApp.Web
{
    public partial class CustomerPage : Page
    {
        private static readonly ILog _log = LogManager.GetLogger(typeof(CustomerPage));

        protected void Page_Load(object sender, EventArgs e)
        {
            if (!IsPostBack)
            {
                BindCustomerGrid();
            }
        }

        private void BindCustomerGrid()
        {
            string connStr = System.Configuration.ConfigurationManager
                .ConnectionStrings["LegacyDb"].ConnectionString;

            using (CustomerRepository repo = new CustomerRepository(connStr))
            {
                List<Customer> customers = repo.GetAll();
                // GridView1.DataSource = customers;
                // GridView1.DataBind();
                _log.InfoFormat("Loaded {0} customers", customers.Count);
            }
        }

        protected void BtnSearch_Click(object sender, EventArgs e)
        {
            // string searchTerm = TxtSearch.Text;
            string searchTerm = "test";
            string connStr = System.Configuration.ConfigurationManager
                .ConnectionStrings["LegacyDb"].ConnectionString;

            using (CustomerRepository repo = new CustomerRepository(connStr))
            {
                List<Customer> results = repo.FindByName(searchTerm);
                // GridView1.DataSource = results;
                // GridView1.DataBind();

                if (results.Count == 0)
                {
                    // LblStatus.Text = "No customers found.";
                    // LblStatus.Visible = true;
                    _log.Info("No customers found for search");
                }
            }
        }

        protected void GridView1_RowCommand(object sender, EventArgs e)
        {
            // Old-school GridView event handling
            _log.Info("GridView row command fired");
        }

        protected void BtnExport_Click(object sender, EventArgs e)
        {
            HttpResponse response = HttpContext.Current.Response;
            response.Clear();
            response.ContentType = "application/vnd.ms-excel";
            response.AddHeader("Content-Disposition", "attachment; filename=customers.xls");

            // Typical 2010-era Excel export via Response.Write
            response.Write("<table>");
            response.Write("<tr><th>Name</th><th>Email</th></tr>");

            string connStr = System.Configuration.ConfigurationManager
                .ConnectionStrings["LegacyDb"].ConnectionString;

            using (CustomerRepository repo = new CustomerRepository(connStr))
            {
                foreach (Customer c in repo.GetAll())
                {
                    response.Write(string.Format(
                        "<tr><td>{0}</td><td>{1}</td></tr>",
                        HttpUtility.HtmlEncode(c.Name),
                        HttpUtility.HtmlEncode(c.Email)));
                }
            }

            response.Write("</table>");
            response.End();
        }
    }
}
